//! Directory transfers: what is planned, and the handle the UI starts, watches
//! and cancels.

use super::*;

/// One file a directory transfer will move. The size is known before anything
/// starts, which is what lets the queue show aggregate progress up front.
#[derive(Clone)]
pub struct PlannedFile {
    pub local: PathBuf,
    pub remote: String,
    pub size: u64,
}

/// A local tree walk: directories parent-first, then files with their sizes.
pub(super) type LocalWalk = (Vec<PathBuf>, Vec<(PathBuf, u64)>);

/// Walks a local tree, returning its directories parent-first and then its files.
///
/// Symlinks are neither followed nor copied: a link is not the file it points at,
/// and following one walks out of the tree or loops.
pub(super) fn walk_local(root: &Path) -> Result<LocalWalk> {
    let mut directories = Vec::new();
    let mut files = Vec::new();
    // Breadth-first, so a directory always appears before its children.
    let mut queue = std::collections::VecDeque::from([root.to_path_buf()]);
    while let Some(dir) = queue.pop_front() {
        for entry in
            std::fs::read_dir(&dir).with_context(|| format!("无法读取 {}", dir.display()))?
        {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            if kind.is_dir() {
                directories.push(path.clone());
                queue.push_back(path);
            } else if kind.is_file() {
                files.push((path, entry.metadata()?.len()));
            }
        }
    }
    Ok((directories, files))
}

/// Turns a path below the source root into the matching path below the target
/// root, in the separator the far end expects.
pub(super) fn relative_to(root: &Path, path: &Path, remote: bool) -> Result<String> {
    let relative = path.strip_prefix(root).context("路径不在源根之下")?;
    let text = relative.to_string_lossy();
    Ok(if remote {
        text.replace('\\', "/")
    } else {
        text.into_owned()
    })
}

/// Walks a local directory and creates the matching remote directories, returning
/// the files to queue.
pub async fn plan_upload(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    local_root: &Path,
    remote_root: &str,
) -> Result<Vec<PlannedFile>> {
    // The walk is synchronous and can cover a whole tree. On a two-worker
    // runtime that would stall SSH and SFTP work, so it goes to the blocking
    // pool rather than occupying a worker.
    let (directories, files) = tokio::task::spawn_blocking({
        let root = local_root.to_path_buf();
        move || walk_local(&root)
    })
    .await
    .context("遍历本地目录的任务失败")??;
    if !sftp.try_exists(remote_root).await? {
        sftp.create_dir(remote_root).await?;
    }
    for dir in &directories {
        let target = join_remote(remote_root, &relative_to(local_root, dir, true)?);
        if !sftp.try_exists(&target).await? {
            sftp.create_dir(&target).await?;
        }
    }
    files
        .into_iter()
        .map(|(path, size)| {
            Ok(PlannedFile {
                remote: join_remote(remote_root, &relative_to(local_root, &path, true)?),
                local: path,
                size,
            })
        })
        .collect()
}

/// Walks a remote directory and creates the matching local directories. The walk
/// doubles as the mkdir pass, so a directory always exists before its children
/// are queued.
pub async fn plan_download(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    remote_root: &str,
    local_root: &Path,
) -> Result<Vec<PlannedFile>> {
    std::fs::create_dir_all(local_root)
        .with_context(|| format!("无法创建 {}", local_root.display()))?;
    let mut files = Vec::new();
    // Each level carries its own local counterpart: joining onto the root every
    // time would flatten the tree and put a nested file beside the top one.
    let mut queue =
        std::collections::VecDeque::from([(remote_root.to_string(), local_root.to_path_buf())]);
    while let Some((dir, local_dir)) = queue.pop_front() {
        for entry in sftp.read_dir(&dir).await? {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            // The server picks these names, and a recursive download must not
            // build a path the local filesystem cannot hold.
            ensure!(safe_local_name(&name), "服务器返回了非法文件名：{name}");
            let remote = join_remote(&dir, &name);
            let local = local_dir.join(&name);
            let kind = entry.file_type();
            if kind.is_symlink() {
                // Same reasoning as the recursive delete: unlink semantics, never
                // follow, or a link back up the tree walks forever.
                continue;
            }
            if kind.is_dir() {
                std::fs::create_dir_all(&local)
                    .with_context(|| format!("无法创建 {}", local.display()))?;
                queue.push_back((remote, local));
            } else {
                let size = entry.metadata().size.unwrap_or(0);
                files.push(PlannedFile {
                    local,
                    remote,
                    size,
                });
            }
        }
    }
    Ok(files)
}

/// Renames a remote path, refusing to replace an existing target.
pub async fn rename(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    from: &str,
    to: &str,
) -> Result<()> {
    ensure!(!sftp.try_exists(to).await?, "目标已存在，请换一个名字");
    sftp.rename(from, to).await?;
    Ok(())
}

pub async fn create_dir(sftp: &Arc<russh_sftp::client::SftpSession>, path: &str) -> Result<()> {
    ensure!(!sftp.try_exists(path).await?, "目标已存在");
    sftp.create_dir(path).await?;
    Ok(())
}

/// Copies one file between the two ends without going through the queue, for the
/// open/edit round-trip. Takes the same transfer slot the queue uses, so an edit
/// save cannot run alongside a bulk transfer.
pub(super) async fn move_one(
    connection: &Arc<Connection>,
    local: &Path,
    remote: &str,
    direction: Direction,
    overwrite: bool,
    action: &str,
) -> Result<()> {
    let sftp = connection.sftp(action).await?;
    let _permit = connection.transfer_gate.clone().acquire_owned().await?;
    let state = Arc::new(Mutex::new(TransferState {
        done: 0,
        total: 0,
        message: String::new(),
        finished: false,
        running: true,
    }));
    let control = TransferControl {
        pause: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        skipped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        overwrite,
        // A one-off copy is its own batch with no UI watching, so nothing can be
        // asked and every conflict has to be settled by `overwrite` up front.
        policy: Arc::new(Mutex::new(BatchPolicy::default())),
        conflicts: mpsc::unbounded_channel().0,
        batch_size: 1,
        batch: 0,
    };
    // These run on the caller's task, which already requests its own repaints.
    let wake: Wake = Arc::new(|| {});
    transfer_file(&sftp, local, remote, direction, &state, &control, &wake).await
}

/// Fetches a remote file to a local path, for opening or editing it.
pub async fn fetch(connection: &Arc<Connection>, remote: &str, local: &Path) -> Result<()> {
    move_one(
        connection,
        local,
        remote,
        Direction::Download,
        false,
        "读取文件",
    )
    .await
}

/// Sends an edited copy back over the remote original. This is the one transfer
/// that is allowed to replace an existing file.
pub async fn put(connection: &Arc<Connection>, remote: &str, local: &Path) -> Result<()> {
    move_one(
        connection,
        local,
        remote,
        Direction::Upload,
        true,
        "回写文件",
    )
    .await
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Direction {
    Upload,
    Download,
}
#[derive(Clone, Debug)]
pub struct TransferState {
    pub done: u64,
    pub total: u64,
    pub message: String,
    pub finished: bool,
    pub running: bool,
}
pub struct Transfer {
    pub local: PathBuf,
    pub remote: String,
    pub direction: Direction,
    pub state: Arc<Mutex<TransferState>>,
    pub pause: Arc<std::sync::atomic::AtomicBool>,
    /// Set by the queue's cancel button. Unlike `pause`, a cancelled transfer ends
    /// for good: it will not offer to resume.
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
    /// Set once this entry holds the transfer slot, so the queue can tell apart
    /// "running" from "queued behind something else".
    pub active: Arc<std::sync::atomic::AtomicBool>,
    /// Raised when the entry settles, so the destination pane is refreshed exactly
    /// once per finish rather than on every repaint.
    pub settled: Arc<std::sync::atomic::AtomicBool>,
    /// When it settled. The queue lets a finished row linger long enough to read
    /// and then drops it, rather than leaving it on screen for the session.
    pub settled_at: Arc<Mutex<Option<std::time::Instant>>>,
    /// Bytes this entry will move, known from the directory walk before any
    /// transfer starts. Feeds the batch's aggregate progress.
    pub planned: u64,
    /// The user action this entry came from. "Apply to all" answers are scoped to
    /// it, so the next directory the user picks asks afresh.
    pub batch: u64,
    /// Shared with every other entry of the same batch.
    pub policy: Arc<Mutex<BatchPolicy>>,
    /// Set when the user chose 跳过, so the queue marks the entry done instead of
    /// offering to resume something that was deliberately left alone.
    skipped: Arc<std::sync::atomic::AtomicBool>,
    /// Replace an existing target without asking, as the edit round-trip does.
    pub(super) overwrite: bool,
    pub connection: Arc<Connection>,
    batch_size: usize,
    pub(super) conflicts: mpsc::UnboundedSender<Conflict>,
}

/// A destination that already exists, parked for the UI to resolve.
pub struct Conflict {
    /// Name shown to the user.
    pub name: String,
    pub existing: u64,
    pub incoming: u64,
    /// How many files the batch covers, so "apply to all" is not a leap of faith.
    pub batch_size: usize,
    /// Which batch asked, so 取消剩余 can stop the rest of it immediately rather
    /// than waiting for each entry to reach its first checkpoint.
    pub batch: u64,
    pub answer: Option<oneshot::Sender<ConflictChoice>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConflictChoice {
    Overwrite,
    OverwriteAll,
    Rename,
    RenameAll,
    Skip,
    SkipAll,
    CancelRemaining,
}

/// Answers already given for one batch, so later files in it are resolved without
/// asking again. Scoped per batch on purpose: a later directory asks afresh.
#[derive(Default)]
pub struct BatchPolicy {
    pub(super) overwrite_all: bool,
    pub(super) rename_all: bool,
    pub(super) skip_all: bool,
    pub(super) cancelled: bool,
}

impl BatchPolicy {
    /// True once the user has abandoned the rest of this batch.
    pub fn cancelled(&self) -> bool {
        self.cancelled
    }
    pub fn cancel_remaining(&mut self) {
        self.cancelled = true;
    }
    /// The "apply to all" answers, so the remaining files of a batch settle
    /// without asking again.
    pub fn overwrite_all(&mut self) {
        self.overwrite_all = true;
    }
    pub fn rename_all(&mut self) {
        self.rename_all = true;
    }
    pub fn skip_all(&mut self) {
        self.skip_all = true;
    }
}

/// Everything needed to put one file in the queue.
pub struct TransferSpec {
    pub local: PathBuf,
    pub remote: String,
    pub direction: Direction,
    /// Bytes this entry will move, from the directory walk.
    pub planned: u64,
    pub batch: u64,
    pub batch_size: usize,
    pub policy: Arc<Mutex<BatchPolicy>>,
    /// Replace an existing target silently. Only the edit round-trip sets this.
    pub overwrite: bool,
}

/// What a running transfer should do at each checkpoint.
pub(super) struct TransferControl {
    pub(super) pause: Arc<std::sync::atomic::AtomicBool>,
    pub(super) cancel: Arc<std::sync::atomic::AtomicBool>,
    pub(super) skipped: Arc<std::sync::atomic::AtomicBool>,
    /// Allow replacing an existing target. Normal transfers ask first; the edit
    /// round-trip needs a silent replace, since every save overwrites the same file.
    pub(super) overwrite: bool,
    pub(super) policy: Arc<Mutex<BatchPolicy>>,
    pub(super) conflicts: mpsc::UnboundedSender<Conflict>,
    pub(super) batch_size: usize,
    pub(super) batch: u64,
}

impl TransferControl {
    pub(super) fn checkpoint(&self) -> Result<()> {
        use std::sync::atomic::Ordering;
        ensure!(!self.cancel.load(Ordering::Acquire), "已取消");
        ensure!(!self.pause.load(Ordering::Acquire), "已暂停");
        Ok(())
    }
}

impl Transfer {
    pub fn new(
        connection: Arc<Connection>,
        spec: TransferSpec,
        conflicts: mpsc::UnboundedSender<Conflict>,
        wake: Wake,
    ) -> Self {
        let transfer = Self {
            local: spec.local,
            remote: spec.remote,
            direction: spec.direction,
            connection,
            state: Arc::new(Mutex::new(TransferState {
                done: 0,
                total: 0,
                message: "排队中".into(),
                finished: false,
                running: false,
            })),
            pause: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            active: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            settled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            settled_at: Arc::new(Mutex::new(None)),
            skipped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            planned: spec.planned,
            batch: spec.batch,
            policy: spec.policy,
            overwrite: spec.overwrite,
            batch_size: spec.batch_size,
            conflicts,
        };
        transfer.start(wake);
        transfer
    }
    /// Stops the transfer for good. Partial data stays on disk, so re-queuing the
    /// same file still offers to resume — cancelling discards the queue entry, not
    /// the bytes already moved.
    pub fn cancel(&self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Release);
    }
    pub fn start(&self, wake: Wake) {
        if self.state.lock().unwrap().running {
            return;
        }
        use std::sync::atomic::Ordering;
        self.pause.store(false, Ordering::Release);
        self.cancel.store(false, Ordering::Release);
        self.skipped.store(false, Ordering::Release);
        self.active.store(false, Ordering::Release);
        self.state.lock().unwrap().running = true;
        let control = TransferControl {
            pause: self.pause.clone(),
            cancel: self.cancel.clone(),
            skipped: self.skipped.clone(),
            overwrite: self.overwrite,
            policy: self.policy.clone(),
            conflicts: self.conflicts.clone(),
            batch_size: self.batch_size,
            batch: self.batch,
        };
        let active = self.active.clone();
        let settled = self.settled.clone();
        let settled_at = self.settled_at.clone();
        let (connection, local, remote, direction, state) = (
            self.connection.clone(),
            self.local.clone(),
            self.remote.clone(),
            self.direction,
            self.state.clone(),
        );
        runtime().spawn(async move {
            let result = async {
                // Resolve SFTP before taking the transfer slot. The lazy handshake
                // can wait out its timeout, and a queued transfer must not spend
                // that time holding the one permit.
                let sftp = connection.sftp("传输文件").await?;
                let _permit = connection.transfer_gate.clone().acquire_owned().await?;
                // Past the gate means this is the one actually moving.
                active.store(true, Ordering::Release);
                transfer_file(&sftp, &local, &remote, direction, &state, &control, &wake).await
            }
            .await;
            let cancelled = control.cancel.load(Ordering::Acquire);
            let skipped = control.skipped.load(Ordering::Acquire);
            active.store(false, Ordering::Release);
            let mut s = state.lock().unwrap();
            s.running = false;
            match result {
                Ok(()) => {
                    s.finished = true;
                    s.message = "完成".into();
                }
                // A cancelled or skipped entry is done, not resumable: the queue
                // row must not offer 继续 for something the user just settled.
                Err(_) if cancelled => {
                    s.finished = true;
                    s.message = "已取消".into();
                }
                Err(_) if skipped => {
                    s.finished = true;
                    s.message = "已跳过".into();
                }
                Err(e) => {
                    s.message = format!("{e:#}");
                }
            }
            drop(s);
            *settled_at.lock().unwrap() = Some(std::time::Instant::now());
            settled.store(true, Ordering::Release);
            wake();
        });
    }
}
