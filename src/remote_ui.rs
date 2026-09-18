use crate::editing;
use crate::theme::Palette;
use eframe::egui::{self, RichText};
use g_terminal::{
    config::RemoteProfile,
    remote::{self, ConnectJob, Connection, Credentials, Direction, Transfer},
    terminal::Terminal,
    ztransfer,
};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
};

pub fn wake(ctx: &egui::Context) -> remote::Wake {
    let ctx = ctx.clone();
    Arc::new(move || ctx.request_repaint())
}
pub fn hint(text: &str, p: Palette) -> RichText {
    RichText::new(text).color(p.muted)
}

pub struct Login {
    pub profile: RemoteProfile,
    credentials: Credentials,
    job: Option<ConnectJob>,
    error: Option<String>,
    /// The passwordless first attempt has not been made yet. Running it here is
    /// what lets a key-only server connect without a click; if it fails, the
    /// credentials simply appear.
    auto: bool,
    /// The current failure came from that automatic attempt, so it is reported as
    /// "needs a password" rather than as an error — needing one is the normal
    /// case, not a fault.
    quiet: bool,
    /// The pane console this connection's failures are mirrored into, so a
    /// message that outlives the dialog still says which session it belongs to.
    console: Option<Arc<Mutex<Terminal>>>,
    /// The last failure already mirrored there.
    noted: Option<String>,
}
impl Login {
    pub fn new(profile: RemoteProfile, console: Option<Arc<Mutex<Terminal>>>) -> Self {
        Self {
            profile,
            credentials: Credentials::default(),
            job: None,
            error: None,
            auto: true,
            quiet: false,
            console,
            noted: None,
        }
    }
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        open: &mut bool,
        p: Palette,
    ) -> Option<Arc<Connection>> {
        // Try once without credentials, so a server that needs none connects on
        // the first click. `ctx` is only in hand here, which is why this is not
        // done in `new`.
        if self.auto {
            self.auto = false;
            self.quiet = true;
            self.job = Some(remote::connect(
                self.profile.clone(),
                Credentials::default(),
                wake(ctx),
            ));
        }
        let mut ready = None;
        let mut abort = false;
        egui::Window::new(format!("SSH · {}", self.profile.label()))
            .open(open)
            .collapsible(false)
            .resizable(false)
            .default_width(410.0)
            .show(ctx, |ui| {
                ui.label(format!(
                    "{}:{}",
                    self.profile.destination(),
                    self.profile.port
                ));
                if let Some(job) = &self.job {
                    let mut state = job.state.lock().unwrap();
                    if let Some(connection) = state.ready.take() {
                        ready = Some(connection);
                    }
                    if let Some(error) = &state.error {
                        self.error = Some(error.clone());
                        // Mirror a real failure into the pane. The passwordless
                        // first attempt failing just means credentials are
                        // needed, which is not an error worth writing down, and
                        // `noted` keeps one failure from being written twice.
                        if !self.quiet && self.noted.as_deref() != Some(error.as_str()) {
                            self.noted = Some(error.clone());
                            if let Some(console) = &self.console {
                                console
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .note_error(&format!("[错误] connect failed: {error}"));
                            }
                        }
                    }
                    if let Some(trust) = &mut state.trust {
                        ui.colored_label(p.accent, "首次连接：确认服务器公钥指纹");
                        ui.label(&trust.host);
                        ui.monospace(&trust.fingerprint);
                        ui.label(hint(
                            "核对指纹后将保存到 known_hosts。主机密钥变化时拒绝连接。",
                            p,
                        ));
                        ui.horizontal(|ui| {
                            if ui.button("信任并继续").clicked()
                                && let Some(answer) = trust.answer.take()
                            {
                                let _ = answer.send(true);
                            }
                            if ui.button("拒绝").clicked()
                                && let Some(answer) = trust.answer.take()
                            {
                                let _ = answer.send(false);
                            }
                        });
                    } else if state.error.is_none() {
                        // A connect can take seconds, and the spinner only turns if
                        // something keeps asking for repaints.
                        ctx.request_repaint_after(std::time::Duration::from_millis(150));
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(&state.message);
                        });
                        if ui.small_button("取消").clicked() {
                            abort = true;
                        }
                    }
                }
                if self.job.is_none() || self.error.is_some() {
                    egui::Grid::new("credentials")
                        .num_columns(2)
                        .spacing([12.0, 6.0])
                        .min_col_width(72.0)
                        .show(ui, |ui| {
                            ui.label("用户名");
                            editing::field_with(ui, &mut self.profile.user, |edit| {
                                edit.desired_width(240.0)
                            });
                            ui.end_row();
                            ui.label("密码");
                            editing::field_with(ui, &mut self.credentials.password, |edit| {
                                edit.password(true).desired_width(240.0)
                            });
                            ui.end_row();
                            ui.label("私钥路径");
                            editing::field_with(ui, &mut self.profile.identity, |edit| {
                                edit.desired_width(240.0)
                            });
                            ui.end_row();
                            ui.label("私钥口令");
                            editing::field_with(ui, &mut self.credentials.passphrase, |edit| {
                                edit.password(true).desired_width(240.0)
                            });
                            ui.end_row();
                            if self.profile.jump.is_some() {
                                ui.label("跳板机密码");
                                editing::field_with(
                                    ui,
                                    &mut self.credentials.jump_password,
                                    |edit| edit.password(true).desired_width(240.0),
                                );
                                ui.end_row();
                            }
                        });
                    if self.quiet {
                        // The passwordless attempt failing usually just means the
                        // server wants credentials, so say so — but the failure
                        // beneath it is still an error and still reads as one.
                        ui.label(hint("需要密码或私钥", p));
                    }
                    if let Some(error) = &self.error {
                        ui.colored_label(p.danger, error);
                    }
                    // Enter submits, matching the other connection dialogs. A
                    // combo/popup consumes Enter for itself first.
                    let enter = !egui::Popup::is_any_open(ui.ctx())
                        && !editing::ime_composing(ui.ctx())
                        && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button("连接").clicked() || enter {
                        self.error = None;
                        self.quiet = false;
                        self.job = Some(remote::connect(
                            self.profile.clone(),
                            std::mem::take(&mut self.credentials),
                            wake(ctx),
                        ));
                    }
                }
            });
        if abort {
            // Dropping the job aborts its task.
            self.job = None;
        }
        ready
    }
}

/// Where a background operation leaves its final message.
type Outcome = Arc<Mutex<Option<Result<String, String>>>>;

/// A ZMODEM transfer the file window keeps on screen while it runs.
struct Operation {
    progress: Arc<Mutex<ztransfer::Progress>>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    result: Outcome,
}

impl Operation {
    fn state(&self) -> ztransfer::Progress {
        self.progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    fn outcome(&self) -> Option<Result<String, String>> {
        self.result.lock().unwrap().clone()
    }
    fn running(&self) -> bool {
        self.result.lock().unwrap().is_none()
    }
}

/// One row of the local file table.
struct LocalEntry {
    name: String,
    directory: bool,
    size: u64,
    symlink: bool,
    /// Modification time in Unix seconds; None when unavailable.
    mtime: Option<u64>,
    /// POSIX mode from `lstat`, or None on platforms that have no such bits.
    perms: Option<u32>,
}

/// The local file's POSIX mode. Windows has no equivalent, so the local table
/// and the 属性 dialog leave permissions out there rather than inventing them.
#[cfg(unix)]
fn local_mode(metadata: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(metadata.permissions().mode())
}

#[cfg(not(unix))]
fn local_mode(_metadata: &std::fs::Metadata) -> Option<u32> {
    None
}

/// The local listing, as the worker thread leaves it.
#[derive(Default)]
struct LocalState {
    entries: Vec<LocalEntry>,
    /// Which directory the entries describe, so a slow read cannot overwrite the
    /// result of a later one.
    path: String,
    loading: bool,
    error: Option<String>,
}

/// Reads one directory into sorted rows. Blocking, so it runs off the UI thread.
fn read_local(path: &str) -> Result<Vec<LocalEntry>, String> {
    let mut rows: Vec<LocalEntry> = std::fs::read_dir(path)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|entry| {
            let metadata = entry.metadata().ok();
            LocalEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                directory: metadata.as_ref().is_some_and(|m| m.is_dir()),
                size: metadata.as_ref().map(|m| m.len()).unwrap_or(0),
                symlink: entry.file_type().ok().is_some_and(|t| t.is_symlink()),
                mtime: metadata
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs()),
                perms: metadata.as_ref().and_then(local_mode),
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(rows)
}

/// A remote row plus its full path, which the row itself does not carry.
#[derive(Clone)]
struct RemoteTarget {
    entry: remote::Entry,
    path: String,
}

/// Everything the 属性 dialog shows. Assembled from a row already in hand, so
/// opening it costs no round trip.
#[derive(Clone)]
struct Properties {
    name: String,
    path: String,
    kind: &'static str,
    size: Option<u64>,
    mtime: Option<String>,
    perms: Option<String>,
    /// Present when the mode (and, remotely, the ownership) can be changed.
    /// Local files only get one on platforms with POSIX bits.
    editable: Option<PermissionEdit>,
}

/// A recursive walk's result, left for the UI thread to collect.
type CountSlot = Arc<Mutex<Option<Result<usize, String>>>>;

/// What a permission change applies to: a local path, or a remote entry reached
/// over SFTP.
#[derive(Clone)]
enum PermTarget {
    Local(PathBuf),
    Remote(RemoteTarget),
}

/// The permission editor's state, kept alongside the dialog because egui rebuilds
/// the window every frame.
#[derive(Clone)]
struct PermissionEdit {
    target: PermTarget,
    mode: u32,
    owner: String,
    group: String,
    recursive: bool,
    /// Set once a recursive walk has counted the entries, which is what the
    /// confirmation is about.
    counted: Option<usize>,
}

/// A destructive action held until the user answers.
struct Confirm {
    message: String,
    action: ConfirmAction,
}

/// What a confirmation will carry out. Kept separate from `MenuAction` so that
/// confirming performs the work directly instead of re-entering the menu dispatch,
/// which would just ask for confirmation again.
enum ConfirmAction {
    DeleteLocal(PathBuf),
    DeleteRemote(RemoteTarget),
}

/// A rename in progress: the name in an editable box plus what it applies to.
struct Rename {
    name: String,
    local: Option<PathBuf>,
    remote: Option<RemoteTarget>,
}

/// One user action that put files in the queue: a single file, or a directory.
struct BatchMeta {
    id: u64,
    /// What the user picked, used as the queue group's label.
    name: String,
    /// A directory pick, drawn as a tree with aggregate progress.
    directory: bool,
}

/// Where a directory walk leaves its result for the UI thread to collect.
type PlanSlot = Arc<Mutex<Option<Result<Vec<remote::PlannedFile>, String>>>>;

/// A directory pick whose walk is still running.
struct PendingBatch {
    id: u64,
    name: String,
    direction: Direction,
    plan: PlanSlot,
}

/// One remote file opened in an external editor, polled for saves to send back.
struct EditSession {
    stop: Arc<std::sync::atomic::AtomicBool>,
    /// The scratch copy's directory, removed when the session ends. Without this
    /// every edited file would leave a directory behind under the temp folder.
    dir: PathBuf,
}

/// What a context-menu item asked for. The row loop only records it; the work
/// happens afterwards, where `&mut self` is free and a spawned task can own what
/// it needs.
enum MenuAction {
    OpenLocal(PathBuf),
    OpenRemote(RemoteTarget),
    EditLocal(PathBuf),
    EditRemote(RemoteTarget),
    Upload(PathBuf),
    Download(RemoteTarget),
    RenameLocal(PathBuf),
    RenameRemote(RemoteTarget),
    DeleteLocal(PathBuf),
    DeleteRemote(RemoteTarget),
    /// Copies text and says so, since a clipboard write has no visible effect.
    CopyPath(String),
    Properties(Properties),
}

pub struct Files {
    pub connection: Arc<Connection>,
    directory: Arc<Mutex<remote::DirectoryState>>,
    remote_path: String,
    local_path: String,
    /// Where the pane-level home buttons jump to.
    local_home: PathBuf,
    /// Captured from the first listing, which resolves `.` to the server's home.
    remote_home: Option<String>,
    selected_local: Option<PathBuf>,
    selected_remote: Option<remote::Entry>,
    /// The local listing, filled on a worker thread.
    local: Arc<Mutex<LocalState>>,
    /// How the worker tells the UI there is something new to show.
    wake: remote::Wake,
    name: String,
    error: Option<String>,
    /// Transient confirmation, e.g. "已复制路径".
    notice: Option<String>,
    pub transfers: Vec<Transfer>,
    operation: Option<Operation>,
    /// Conflict a running ZMODEM transfer is waiting on an answer for.
    question: Option<ztransfer::Question>,
    questions: Option<tokio::sync::mpsc::UnboundedReceiver<ztransfer::Question>>,
    answers: Option<tokio::sync::mpsc::UnboundedSender<ztransfer::Decision>>,
    /// Conflicts raised by queued transfers, and the slot the dialog answers from.
    /// One slot is enough: the transfer gate lets only one file move at a time.
    conflicts: tokio::sync::mpsc::UnboundedSender<remote::Conflict>,
    conflict_rx: tokio::sync::mpsc::UnboundedReceiver<remote::Conflict>,
    conflict: Option<remote::Conflict>,
    /// One entry per user action, so the queue can group entries and aggregate.
    batches: Vec<BatchMeta>,
    batch_seq: u64,
    /// A directory pick whose walk is still running.
    pending_batch: Option<PendingBatch>,
    rename: String,
    split_ratio: f32,
    /// Height the footer rows occupied last frame. The panes reserve it so the
    /// window's content never exceeds its own height.
    footer_height: f32,
    show_hidden: bool,
    properties: Option<Properties>,
    /// A recursive-permission walk in flight, counting what it would touch.
    counting: Option<CountSlot>,
    confirm: Option<Confirm>,
    renaming: Option<Rename>,
    /// A directory delete waiting on the server's emptiness verdict.
    deleting: Option<RemoteTarget>,
    /// Live editor round-trips. Dropping `Files` stops them, so a closed window
    /// cannot leave a task holding the connection open.
    edits: Vec<EditSession>,
}

impl Drop for Files {
    fn drop(&mut self) {
        for session in &self.edits {
            session
                .stop
                .store(true, std::sync::atomic::Ordering::Release);
            // The scratch copy has served its purpose; a closed window should not
            // leave one directory per edited file behind.
            let _ = std::fs::remove_dir_all(&session.dir);
        }
    }
}
impl Files {
    pub fn new(connection: Arc<Connection>, ctx: &egui::Context, hide_dotfiles: bool) -> Self {
        let dirs = directories::UserDirs::new();
        let home = dirs.as_ref().map(|d| d.home_dir().to_path_buf());
        // Downloads is where a file browser's traffic lands; the home button still
        // jumps to the home directory, which is a different destination.
        let local_path = dirs
            .as_ref()
            .and_then(|d| d.download_dir().map(|d| d.to_path_buf()))
            .or_else(|| home.clone())
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| ".".into());
        let (conflicts, conflict_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut this = Self {
            directory: remote::list_directory(connection.clone(), ".".into(), wake(ctx)),
            connection,
            remote_path: ".".into(),
            local_path,
            local_home: home.unwrap_or_else(|| PathBuf::from(".")),
            remote_home: None,
            selected_local: None,
            selected_remote: None,
            local: Arc::new(Mutex::new(LocalState::default())),
            wake: wake(ctx),
            name: String::new(),
            error: None,
            notice: None,
            transfers: vec![],
            operation: None,
            question: None,
            questions: None,
            answers: None,
            conflicts,
            conflict_rx,
            conflict: None,
            batches: vec![],
            batch_seq: 0,
            pending_batch: None,
            rename: String::new(),
            split_ratio: 0.5,
            // A first-frame estimate, so the window does not jump before the real
            // footer has been measured once.
            footer_height: 150.0,
            show_hidden: !hide_dotfiles,
            properties: None,
            counting: None,
            confirm: None,
            renaming: None,
            deleting: None,
            edits: vec![],
        };
        this.refresh_local();
        this
    }
    /// Picks up a conflict a running transfer is blocked on.
    fn poll_question(&mut self) {
        if let Some(questions) = &mut self.questions {
            while let Ok(question) = questions.try_recv() {
                self.rename = question.name();
                self.question = Some(question);
            }
        }
    }

    /// True while a ZMODEM transfer wants the window on screen.
    pub fn busy(&self) -> bool {
        self.operation.as_ref().is_some_and(Operation::running)
    }
    /// True while a directory listing is in flight. Opening the window on a server
    /// without SFTP waits out the subsystem probe here, and the spinner needs
    /// repaints to keep turning for that whole time.
    pub fn loading(&self) -> bool {
        // Either pane still filling counts: both drive their own repaints.
        self.directory.lock().unwrap().loading || self.local.lock().unwrap().loading
    }

    /// Asks a running transfer to stop; it aborts the peer and cleans up.
    pub fn cancel(&self) {
        if let Some(operation) = &self.operation {
            operation
                .cancel
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }

    /// Re-reads the local directory on a worker thread.
    ///
    /// This used to run inline, and it is one `read_dir` plus a `stat` and a
    /// `file_type` per entry: pointing the address bar at something like
    /// `C:\Windows\WinSxS` froze the window for as long as that took.
    fn refresh_local(&mut self) {
        let path = self.local_path.clone();
        let state = self.local.clone();
        let wake = self.wake.clone();
        {
            let mut state = state.lock().unwrap();
            state.loading = true;
            state.error = None;
            state.path = path.clone();
        }
        // The selected row belongs to the listing being replaced.
        self.selected_local = None;
        remote::runtime().spawn_blocking(move || {
            let result = read_local(&path);
            let mut state = state.lock().unwrap();
            // A slower listing for an older directory must not overwrite this one.
            if state.path == path {
                match result {
                    Ok(entries) => state.entries = entries,
                    Err(e) => state.error = Some(e),
                }
                state.loading = false;
            }
            wake();
        });
    }
    fn refresh_remote(&mut self, ctx: &egui::Context) {
        self.directory =
            remote::list_directory(self.connection.clone(), self.remote_path.clone(), wake(ctx));
        self.selected_remote = None;
    }
    fn remote_child(&self, name: &str) -> String {
        join_path(&self.directory.lock().unwrap().path, name)
    }

    /// The remote directory the listing is showing, for the drag-and-drop hint.
    pub fn remote_dir(&self) -> String {
        self.directory
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .path
            .clone()
    }

    /// Queues a local file or directory for upload into the directory the
    /// window is showing. Used by files dropped onto the app window.
    pub fn upload_path(&mut self, path: PathBuf, ctx: &egui::Context) {
        let name = file_name_of(&path);
        if name.is_empty() {
            self.error = Some("拖入的路径没有文件名".into());
            return;
        }
        if self.remote_dir().is_empty() {
            self.error = Some("远程目录还没加载完，请稍后再拖入".into());
            return;
        }
        let directory = path.is_dir();
        let remote = self.remote_child(&name);
        self.transfer(path, remote, Direction::Upload, directory, ctx);
    }

    /// Reserves the single operation slot for a short SFTP-side action.
    fn simple(&mut self, name: &str) -> Outcome {
        self.simple_progress(name).1
    }

    /// Like [`Self::simple`], but also hands back the progress slot so a long walk
    /// can report how far it has got. The footer and the top bar both read that
    /// slot, so the bar appears without any further plumbing.
    fn simple_progress(&mut self, name: &str) -> (Arc<Mutex<ztransfer::Progress>>, Outcome) {
        let result = Arc::new(Mutex::new(None));
        let progress = Arc::new(Mutex::new(ztransfer::Progress {
            name: name.into(),
            message: "进行中".into(),
            ..Default::default()
        }));
        self.operation = Some(Operation {
            progress: progress.clone(),
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            result: result.clone(),
        });
        (progress, result)
    }

    /// Reserves the single ZMODEM slot and wires up its question channel.
    /// Refuses while another transfer still owns the slot.
    fn begin(&mut self, name: &str) -> Option<(ztransfer::Handle, Outcome)> {
        if self.busy() {
            self.error = Some("已有传输正在进行，请等待完成或先取消".into());
            return None;
        }
        let (ask, questions, answers) = ztransfer::ask_channel();
        self.questions = Some(questions);
        self.answers = Some(answers);
        self.question = None;
        self.rename.clear();
        let result = Arc::new(Mutex::new(None));
        let handle = ztransfer::Handle::new(name, Some(ask));
        self.operation = Some(Operation {
            progress: handle.progress.clone(),
            cancel: handle.cancel.clone(),
            result: result.clone(),
        });
        Some((handle, result))
    }

    /// Answers and progress of a running ZMODEM transfer, for the status bar.
    pub fn operation_state(&self) -> Option<ztransfer::Progress> {
        self.operation.as_ref().map(Operation::state)
    }

    /// Draws the overwrite / rename / resume prompt for an incoming file.
    pub fn show_question(&mut self, ctx: &egui::Context, p: Palette) {
        self.poll_question();
        let Some(question) = self.question.clone() else {
            return;
        };
        let mut answer = None;
        egui::Window::new("ZMODEM 文件冲突")
            .collapsible(false)
            .resizable(false)
            .default_width(400.0)
            // A prompt the transfer is waiting on must never end up behind the
            // file window, which is an ordinary window and can be raised.
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!(
                    "服务器正在发送 {}（{}）",
                    question.name(),
                    format_size(question.total)
                ));
                if let Some(size) = question.existing {
                    ui.colored_label(
                        p.warn,
                        format!("目标位置已有同名文件，{}", format_size(size)),
                    );
                }
                if let Some(size) = question.partial {
                    ui.label(hint(
                        &format!("存在未完成的下载，可续传（已完成 {}）", format_size(size)),
                        p,
                    ));
                }
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if question.can_resume() && ui.button("续传").clicked() {
                        answer = Some(ztransfer::Decision::Resume);
                    }
                    if ui
                        .button("覆盖")
                        .on_hover_text("传输完成后替换同名文件；中途失败不会破坏原文件")
                        .clicked()
                    {
                        answer = Some(ztransfer::Decision::Overwrite);
                    }
                    editing::field_with(ui, &mut self.rename, |edit| edit.desired_width(150.0));
                    if ui.button("重命名").clicked() {
                        answer = Some(ztransfer::Decision::Rename(self.rename.clone()));
                    }
                    if ui.button("取消传输").clicked() {
                        answer = Some(ztransfer::Decision::Cancel);
                    }
                });
            });
        if let Some(answer) = answer
            && let Some(answers) = &self.answers
        {
            let _ = answers.send(answer);
            self.question = None;
        }
    }
    pub fn start_terminal_zmodem(
        &mut self,
        stream: g_terminal::session::BridgeStream,
        upload: bool,
        local: PathBuf,
        ctx: &egui::Context,
    ) {
        let name = local
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "ZMODEM".into());
        let Some((handle, operation)) = self.begin(&name) else {
            return;
        };
        let wake = wake(ctx);
        remote::runtime().spawn(async move {
            let result: anyhow::Result<String> = async {
                let mut stream = stream;
                if upload {
                    g_terminal::ztransfer::send(&mut stream, &local, &handle).await?;
                    Ok("ZMODEM 上传完成".into())
                } else {
                    let saved =
                        g_terminal::ztransfer::receive(&mut stream, &local, &handle).await?;
                    Ok(format!("ZMODEM 已保存到 {}", saved.display()))
                }
            }
            .await;
            *operation.lock().unwrap() = Some(result.map_err(|e| format!("{e:#}")));
            wake();
        });
    }
    fn next_batch_id(&mut self) -> u64 {
        self.batch_seq += 1;
        self.batch_seq
    }
    fn queue(&mut self, local: PathBuf, remote: String, direction: Direction, ctx: &egui::Context) {
        // The edit round-trip re-uploads the same path on every save, so it is the
        // one caller that may replace an existing target.
        self.queue_replacing(local, remote, direction, false, ctx);
    }
    fn queue_replacing(
        &mut self,
        local: PathBuf,
        remote: String,
        direction: Direction,
        overwrite: bool,
        ctx: &egui::Context,
    ) {
        if self.is_queued(&local, &remote) {
            self.error = Some("该文件已在队列中，请使用继续按钮".into());
            return;
        }
        let planned = std::fs::metadata(&local).map(|m| m.len()).unwrap_or(0);
        let id = self.next_batch_id();
        self.batches.push(BatchMeta {
            id,
            name: file_name_of(&local),
            directory: false,
        });
        self.push_transfer(
            remote::TransferSpec {
                local,
                remote,
                direction,
                planned,
                batch: id,
                batch_size: 1,
                policy: Arc::new(Mutex::new(remote::BatchPolicy::default())),
                overwrite,
            },
            ctx,
        );
    }
    fn is_queued(&self, local: &Path, remote: &str) -> bool {
        self.transfers
            .iter()
            .any(|t| t.local == local && t.remote == remote && !t.state.lock().unwrap().finished)
    }
    fn push_transfer(&mut self, spec: remote::TransferSpec, ctx: &egui::Context) {
        self.transfers.push(Transfer::new(
            self.connection.clone(),
            spec,
            self.conflicts.clone(),
            wake(ctx),
        ));
    }
    /// Queues a file, or walks a directory and queues everything under it.
    fn transfer(
        &mut self,
        local: PathBuf,
        remote: String,
        direction: Direction,
        directory: bool,
        ctx: &egui::Context,
    ) {
        if directory {
            self.start_batch(file_name_of(&local), local, remote, direction, ctx);
        } else {
            self.queue(local, remote, direction, ctx);
        }
    }
    /// Starts a directory transfer. The walk runs off the UI thread; the entries it
    /// finds are queued once it lands, which is why this returns immediately.
    fn start_batch(
        &mut self,
        name: String,
        local_root: PathBuf,
        remote_root: String,
        direction: Direction,
        ctx: &egui::Context,
    ) {
        let id = self.next_batch_id();
        let plan: PlanSlot = Arc::new(Mutex::new(None));
        let slot = plan.clone();
        let connection = self.connection.clone();
        let wake = wake(ctx);
        remote::runtime().spawn(async move {
            let result = async {
                let sftp = connection.sftp("遍历目录").await?;
                match direction {
                    Direction::Upload => {
                        remote::plan_upload(&sftp, &local_root, &remote_root).await
                    }
                    Direction::Download => {
                        remote::plan_download(&sftp, &remote_root, &local_root).await
                    }
                }
            }
            .await;
            *slot.lock().unwrap() = Some(result.map_err(|e| format!("{e:#}")));
            wake();
        });
        self.pending_batch = Some(PendingBatch {
            id,
            name,
            direction,
            plan,
        });
    }
    /// Turns a finished directory walk into queue entries.
    fn poll_batch(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.pending_batch else {
            return;
        };
        let planned = pending.plan.lock().unwrap().clone();
        let Some(planned) = planned else {
            return;
        };
        let Some(pending) = self.pending_batch.take() else {
            return;
        };
        let files = match planned {
            Ok(files) => files,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        if files.is_empty() {
            self.notice = Some(format!("{} 里没有文件", pending.name));
            return;
        }
        let size = files.len();
        self.batches.push(BatchMeta {
            id: pending.id,
            name: pending.name,
            directory: true,
        });
        // One policy for the whole batch, so an "apply to all" answer covers it and
        // stops at its edge.
        let policy = Arc::new(Mutex::new(remote::BatchPolicy::default()));
        for file in files {
            // A destination already queued is skipped silently; a whole directory
            // would otherwise raise one error per entry.
            if self.is_queued(&file.local, &file.remote) {
                continue;
            }
            self.push_transfer(
                remote::TransferSpec {
                    local: file.local,
                    remote: file.remote,
                    direction: pending.direction,
                    planned: file.size,
                    batch: pending.id,
                    batch_size: size,
                    policy: policy.clone(),
                    overwrite: false,
                },
                ctx,
            );
        }
    }
    /// Picks up a conflict a queued transfer is blocked on.
    fn poll_conflict(&mut self) {
        while let Ok(conflict) = self.conflict_rx.try_recv() {
            // One at a time is all the transfer gate allows, so the newest is the
            // only live one.
            self.conflict = Some(conflict);
        }
    }
    /// The conflict dialog. Answers the transfer waiting on it, then forgets it.
    fn show_conflict(&mut self, ctx: &egui::Context, p: Palette) {
        let Some(conflict) = &self.conflict else {
            return;
        };
        let (name, existing, incoming, batch_size, batch) = (
            conflict.name.clone(),
            conflict.existing,
            conflict.incoming,
            conflict.batch_size,
            conflict.batch,
        );
        let mut answer = None;
        // A modal, not a window: the transfer is blocked until this is answered,
        // and a window that shares the file window's grey made it easy to miss.
        // The backdrop dims everything behind it, so there is no doubt about
        // what is being asked or where the answer goes.
        // A modal with a real header and a warn-coloured edge: the transfer is
        // blocked until this is answered, so it has to read as one dialog rather
        // than as a heap of equally sized lines and buttons.
        let frame = egui::Frame::popup(&ctx.style())
            .stroke(egui::Stroke::new(1.0_f32, p.warn.gamma_multiply(0.7)));
        egui::Modal::new(egui::Id::new("sftp-transfer-conflict"))
            .frame(frame)
            .show(ctx, |ui| {
                answer = conflict_body(ui, &name, existing, incoming, batch_size, p);
            });
        let Some(choice) = answer else {
            return;
        };
        if let Some(mut conflict) = self.conflict.take()
            && let Some(sender) = conflict.answer.take()
        {
            let _ = sender.send(choice);
        }
        // 取消剩余 has to stop entries that have not reached the front of the queue
        // yet, not just the one that asked: the policy only applies at the next
        // checkpoint, which a queued entry has not reached.
        if choice == remote::ConflictChoice::CancelRemaining {
            for transfer in &self.transfers {
                if transfer.batch == batch {
                    transfer.cancel();
                }
            }
        }
    }
    /// Refreshes the destination pane once a transfer settles, so a drop shows up
    /// without pressing 刷新.
    fn poll_transfers(&mut self, ctx: &egui::Context) {
        use std::sync::atomic::Ordering;
        let mut local_dir = None;
        let mut remote_dir = None;
        for transfer in &self.transfers {
            if !transfer.settled.swap(false, Ordering::AcqRel) {
                continue;
            }
            match transfer.direction {
                Direction::Upload => {
                    let dir = parent_path(&transfer.remote);
                    // Wait for anything else heading for the same directory, so a
                    // directory transfer refreshes once at the end instead of
                    // clearing and reloading the list once per file.
                    let busy = self.transfers.iter().any(|other| {
                        matches!(other.direction, Direction::Upload)
                            && parent_path(&other.remote) == dir
                            && other.state.lock().unwrap().running
                    });
                    if !busy {
                        remote_dir = Some(dir);
                    }
                }
                Direction::Download => {
                    let Some(dir) = transfer.local.parent().map(Path::to_path_buf) else {
                        continue;
                    };
                    let busy = self.transfers.iter().any(|other| {
                        matches!(other.direction, Direction::Download)
                            && other.local.parent() == Some(dir.as_path())
                            && other.state.lock().unwrap().running
                    });
                    if !busy {
                        local_dir = Some(dir);
                    }
                }
            }
        }
        // Only if the user is still looking at that directory.
        if let Some(dir) = remote_dir
            && self.directory.lock().unwrap().path == dir
        {
            self.refresh_remote(ctx);
        }
        if let Some(dir) = local_dir
            && Path::new(&self.local_path) == dir.as_path()
        {
            self.refresh_local();
        }
        self.prune_queue();
        // A settled row disappears on a timer, so the frame that notices has to be
        // asked for — an idle window would otherwise leave it on screen.
        if !self.transfers.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }
    /// Drops settled queue entries once they are old news.
    ///
    /// Nothing removed them before, and each one holds an `Arc<Connection>` and a
    /// handful of `Arc`s beyond that — a directory transfer would pin thousands of
    /// them, and the connection with them, for the life of the window.
    fn prune_queue(&mut self) {
        /// A backstop for pathological queues, regardless of age.
        const KEEP_SETTLED: usize = 40;
        /// How long a finished row stays on screen. Long enough to read the
        /// outcome, short enough that a cancelled transfer does not sit there for
        /// the rest of the session.
        const LINGER: std::time::Duration = std::time::Duration::from_secs(6);
        let now = std::time::Instant::now();
        // A batch with entries still running keeps all of them: the group's
        // progress bar sums its members, so dropping a finished one would make the
        // bar run backwards.
        let unfinished: std::collections::HashSet<u64> = self
            .transfers
            .iter()
            .filter(|transfer| !transfer.state.lock().unwrap().finished)
            .map(|transfer| transfer.batch)
            .collect();
        let finished: Vec<usize> = self
            .transfers
            .iter()
            .enumerate()
            .filter(|(_, transfer)| transfer.state.lock().unwrap().finished)
            .map(|(index, _)| index)
            .collect();
        let mut doomed: Vec<usize> = finished
            .iter()
            .copied()
            .filter(|index| {
                let transfer = &self.transfers[*index];
                if unfinished.contains(&transfer.batch) {
                    return false;
                }
                transfer
                    .settled_at
                    .lock()
                    .unwrap()
                    .is_some_and(|at| now.duration_since(at) >= LINGER)
            })
            .collect();
        // Anything past the cap goes immediately, however recently it settled.
        doomed.extend(settled_to_drop(&finished, KEEP_SETTLED));
        doomed.sort_unstable();
        doomed.dedup();
        if doomed.is_empty() {
            return;
        }
        // Removing by index in reverse keeps the remaining indices valid.
        for index in doomed.into_iter().rev() {
            self.transfers.remove(index);
        }
        // A batch with no members left would render as nothing, but its id would
        // keep the sequence growing.
        let live: std::collections::HashSet<u64> = self
            .transfers
            .iter()
            .map(|transfer| transfer.batch)
            .collect();
        self.batches.retain(|batch| live.contains(&batch.id));
    }
    fn entry_color(
        directory: bool,
        symlink: bool,
        executable: bool,
        p: Palette,
    ) -> eframe::egui::Color32 {
        // Blue, cyan, green — the `ls` convention, and the terminal's own colours,
        // so the list and `ls` agree. Directories used to be the accent green,
        // which reads as "executable" to anyone who has used `ls --color`.
        if symlink {
            p.symlink
        } else if directory {
            p.directory
        } else if executable {
            p.executable
        } else {
            p.text
        }
    }
    fn perms_string(perms: Option<u32>) -> String {
        perms.map_or_else(String::new, |m| {
            format!(
                "{}{}{}{}{}{}{}{}{}{}",
                if m & 0o400 != 0 { 'r' } else { '-' },
                if m & 0o200 != 0 { 'w' } else { '-' },
                if m & 0o100 != 0 { 'x' } else { '-' },
                if m & 0o040 != 0 { 'r' } else { '-' },
                if m & 0o020 != 0 { 'w' } else { '-' },
                if m & 0o010 != 0 { 'x' } else { '-' },
                if m & 0o004 != 0 { 'r' } else { '-' },
                if m & 0o002 != 0 { 'w' } else { '-' },
                if m & 0o001 != 0 { 'x' } else { '-' },
                if m & 0o1000 != 0 { 't' } else { ' ' },
            )
        })
    }
    fn show_local_pane(
        &mut self,
        ui: &mut egui::Ui,
        p: Palette,
        next: &mut Option<String>,
        menu: &mut Option<MenuAction>,
    ) {
        ui.horizontal(|ui| {
            // Laid out from the right so the field takes exactly what is left.
            // Sizing it from the left pushed 刷新 past the pane edge, where the
            // divider or the window edge cut it off.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let refresh =
                    crate::icons::icon_label_button(ui, crate::icons::Icon::Refresh, "刷新", p);
                // The two icons come after the field in this right-to-left layout,
                // so their room has to be held back: otherwise the field takes the
                // lot and they land at negative x, over the pane to the left.
                let icons =
                    crate::icons::Size::Button.button().x * 2.0 + ui.spacing().item_spacing.x * 2.0;
                let room = ui.available_width();
                // They are shortcuts; the field is not. In a pane too narrow for
                // both, the shortcuts go, so that nothing is ever pushed outside —
                // a floor on the field's width would guarantee that it was.
                let shortcuts = room - icons >= MIN_FIELD_WIDTH;
                let edit = editing::field_with(ui, &mut self.local_path, |edit| {
                    edit.desired_width(if shortcuts { room - icons } else { room })
                });
                if shortcuts {
                    if crate::icons::icon_button(
                        ui,
                        crate::icons::Icon::Home,
                        p,
                        crate::icons::Size::Button,
                    )
                    .on_hover_text("主目录")
                    .clicked()
                    {
                        *next = Some(self.local_home.display().to_string());
                    }
                    if crate::icons::icon_button(
                        ui,
                        crate::icons::Icon::FolderUp,
                        p,
                        crate::icons::Size::Button,
                    )
                    .on_hover_text("上一级")
                    .clicked()
                    {
                        *next = PathBuf::from(&self.local_path)
                            .parent()
                            .map(|p| p.display().to_string());
                    }
                }
                if refresh.clicked()
                    || (edit.lost_focus()
                        && !editing::ime_composing(ui.ctx())
                        && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                {
                    *next = Some(self.local_path.clone());
                }
            });
        });
        // Holding the Arc rather than borrowing `self`, so the row handlers below
        // stay free to write `selected_local`.
        let local = self.local.clone();
        let local = local.lock().unwrap();
        if local.loading {
            ui.spinner();
        }
        if let Some(e) = &local.error {
            ui.colored_label(p.danger, e);
        }
        // The header lives inside the scroller, so it travels sideways with the
        // rows instead of drifting out of alignment with them.
        egui::ScrollArea::both()
            .id_salt("local-files")
            .auto_shrink([false, false])
            .max_height(list_height(ui))
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                // Wider than the pane when the pane is narrow: the columns keep
                // their widths and the overflow becomes a horizontal scrollbar,
                // rather than every column being squeezed into a truncation.
                let width = ui.available_width().max(MIN_TABLE_WIDTH);
                table_header(ui, p, &LOCAL_COLUMNS, &LOCAL_HEADERS, width);
                for entry in &local.entries {
                    if !self.show_hidden && entry.name.starts_with('.') {
                        continue;
                    }
                    let path = PathBuf::from(&self.local_path).join(&entry.name);
                    // A set executable bit (any of the three) is what `ls`
                    // highlights, and what makes the icon a script rather than a
                    // document. A symlink's own mode says nothing about its
                    // target, so links never count as executable here; Windows
                    // reports no mode at all.
                    let executable = !entry.symlink && entry.perms.is_some_and(|m| m & 0o111 != 0);
                    let mut cells = vec![
                        Cell {
                            text: format!(
                                "{}{}",
                                entry.name,
                                entry_suffix(entry.directory, entry.symlink)
                            ),
                            right: false,
                            color: Self::entry_color(entry.directory, entry.symlink, executable, p),
                            full: None,
                            icon: Some(file_icon(&entry.name, entry.directory, executable)),
                            link: entry.symlink,
                        },
                        Cell {
                            // A directory has no meaningful size of its own.
                            text: if entry.directory {
                                String::new()
                            } else {
                                format_size(entry.size)
                            },
                            right: true,
                            color: p.muted,
                            full: None,
                            icon: None,
                            link: false,
                        },
                    ];
                    #[cfg(unix)]
                    cells.push(Cell {
                        text: Self::perms_string(entry.perms),
                        right: true,
                        color: p.muted,
                        full: None,
                        icon: None,
                        link: false,
                    });
                    cells.push(Cell {
                        text: entry.mtime.map_or_else(String::new, format_time),
                        right: true,
                        color: p.muted,
                        // The column shows a shortened form; hovering recovers
                        // the exact timestamp.
                        full: entry.mtime.map(format_time_full),
                        icon: None,
                        link: false,
                    });
                    let selected = self.selected_local.as_ref() == Some(&path);
                    let (r, _) = table_row(ui, &cells, &LOCAL_COLUMNS, selected, width);
                    // A right-click selects the row first, the way a file manager
                    // does, so the menu always acts on the row under the cursor
                    // rather than on whatever happened to be selected.
                    if r.secondary_clicked() {
                        self.selected_local = Some(path.clone());
                    }
                    if r.clicked() {
                        self.selected_local = Some(path.clone());
                    }
                    if r.double_clicked() {
                        if entry.directory {
                            *next = Some(path.display().to_string());
                        } else {
                            // A file opens in whatever the OS associates with it,
                            // the same as the menu's 打开.
                            *menu = Some(MenuAction::OpenLocal(path.clone()));
                        }
                    }
                    r.context_menu(|ui| {
                        context_menu(
                            ui,
                            menu,
                            vec![
                                menu_item("打开", true, MenuAction::OpenLocal(path.clone())),
                                menu_item(
                                    "编辑",
                                    !entry.directory,
                                    MenuAction::EditLocal(path.clone()),
                                ),
                                separator(),
                                menu_item(
                                    "上传 →",
                                    !entry.directory,
                                    MenuAction::Upload(path.clone()),
                                ),
                                separator(),
                                menu_item("重命名", true, MenuAction::RenameLocal(path.clone())),
                                menu_item("删除", true, MenuAction::DeleteLocal(path.clone())),
                                separator(),
                                menu_item(
                                    "复制路径",
                                    true,
                                    MenuAction::CopyPath(path.display().to_string()),
                                ),
                                menu_item(
                                    "属性",
                                    true,
                                    MenuAction::Properties(local_properties(entry, &path)),
                                ),
                            ],
                        );
                    });
                }
            });
    }
    fn show_remote_pane(
        &mut self,
        ui: &mut egui::Ui,
        p: Palette,
        next: &mut Option<String>,
        menu: &mut Option<MenuAction>,
    ) {
        // Snapshot under the lock, then release it before the row handlers below
        // start mutating other `self` fields.
        let (path, loading, error, entries) = {
            let state = self.directory.lock().unwrap();
            (
                state.path.clone(),
                state.loading,
                state.error.clone(),
                state.entries.clone(),
            )
        };
        // The first listing resolves `.` to the server's home directory, which is
        // what the home button jumps back to.
        if self.remote_home.is_none() && !loading && !path.is_empty() {
            self.remote_home = Some(path.clone());
        }
        ui.horizontal(|ui| {
            // Right to left, same as the local pane: the field takes what is left
            // and the buttons cannot be pushed out of the pane.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let refresh =
                    crate::icons::icon_label_button(ui, crate::icons::Icon::Refresh, "刷新", p);
                // Same reservation as the local pane: the icons follow the field,
                // and they are dropped rather than the field overflowing.
                let icons =
                    crate::icons::Size::Button.button().x * 2.0 + ui.spacing().item_spacing.x * 2.0;
                let room = ui.available_width();
                let shortcuts = room - icons >= MIN_FIELD_WIDTH;
                let edit = editing::field_with(ui, &mut self.remote_path, |edit| {
                    edit.desired_width(if shortcuts { room - icons } else { room })
                });
                let home = self.remote_home.clone();
                if shortcuts {
                    let home_button = crate::icons::icon_button(
                        ui,
                        crate::icons::Icon::Home,
                        p,
                        crate::icons::Size::Button,
                    )
                    .on_hover_text("主目录");
                    if let Some(home) = home
                        && home_button.clicked()
                    {
                        *next = Some(home);
                    }
                    if crate::icons::icon_button(
                        ui,
                        crate::icons::Icon::FolderUp,
                        p,
                        crate::icons::Size::Button,
                    )
                    .on_hover_text("上一级")
                    .clicked()
                    {
                        *next = Some(parent_path(&path));
                    }
                }
                // Show the path the server actually resolved, but never overwrite
                // what the user is in the middle of typing.
                if !loading && !path.is_empty() && !edit.has_focus() {
                    self.remote_path.clone_from(&path);
                }
                if refresh.clicked()
                    || (edit.lost_focus()
                        && !editing::ime_composing(ui.ctx())
                        && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                {
                    *next = Some(self.remote_path.clone());
                }
            });
        });
        if loading {
            ui.spinner();
        }
        if let Some(e) = &error {
            ui.colored_label(p.danger, e);
        }
        // The header lives inside the scroller, so it travels sideways with the
        // rows instead of drifting out of alignment with them.
        egui::ScrollArea::both()
            .id_salt("remote-files")
            .auto_shrink([false, false])
            .max_height(list_height(ui))
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                let width = ui.available_width().max(MIN_TABLE_WIDTH);
                table_header(
                    ui,
                    p,
                    &REMOTE_COLUMNS,
                    &["名称", "大小", "权限", "修改时间"],
                    width,
                );
                for entry in &entries {
                    if !self.show_hidden && entry.name.starts_with('.') {
                        continue;
                    }
                    let executable = entry.perms.is_some_and(|m| m & 0o111 != 0);
                    let cells = [
                        Cell {
                            text: format!(
                                "{}{}",
                                entry.name,
                                entry_suffix(entry.directory, entry.symlink)
                            ),
                            right: false,
                            color: Self::entry_color(entry.directory, entry.symlink, executable, p),
                            full: None,
                            icon: Some(file_icon(&entry.name, entry.directory, executable)),
                            link: entry.symlink,
                        },
                        Cell {
                            text: if entry.directory {
                                String::new()
                            } else {
                                format_size(entry.size)
                            },
                            right: true,
                            color: p.muted,
                            full: None,
                            icon: None,
                            link: false,
                        },
                        Cell {
                            text: Self::perms_string(entry.perms),
                            right: true,
                            color: p.muted,
                            full: None,
                            icon: None,
                            link: false,
                        },
                        Cell {
                            text: entry
                                .mtime
                                .map_or_else(String::new, |t| format_time(u64::from(t))),
                            right: true,
                            color: p.muted,
                            full: entry.mtime.map(|t| format_time_full(u64::from(t))),
                            icon: None,
                            link: false,
                        },
                    ];
                    let selected = self
                        .selected_remote
                        .as_ref()
                        .is_some_and(|e| e.name == entry.name);
                    let (r, _) = table_row(ui, &cells, &REMOTE_COLUMNS, selected, width);
                    if r.secondary_clicked() {
                        self.selected_remote = Some(entry.clone());
                    }
                    if r.clicked() {
                        self.selected_remote = Some(entry.clone());
                        self.name = entry.name.clone();
                    }
                    if r.double_clicked() {
                        if entry.directory {
                            *next = Some(join_path(&path, &entry.name));
                        } else {
                            // A remote file is fetched to the scratch directory and
                            // then opened, exactly as the menu's 打开 does.
                            *menu = Some(MenuAction::OpenRemote(RemoteTarget {
                                entry: entry.clone(),
                                path: join_path(&path, &entry.name),
                            }));
                        }
                    }
                    r.context_menu(|ui| {
                        let target = || RemoteTarget {
                            entry: entry.clone(),
                            path: join_path(&path, &entry.name),
                        };
                        // Everything but the metadata actions needs a regular file:
                        // a directory is entered by double-clicking, not opened.
                        let file = !entry.directory;
                        context_menu(
                            ui,
                            menu,
                            vec![
                                menu_item("打开", file, MenuAction::OpenRemote(target())),
                                menu_item("编辑", file, MenuAction::EditRemote(target())),
                                menu_item("← 下载", file, MenuAction::Download(target())),
                                separator(),
                                menu_item("重命名", true, MenuAction::RenameRemote(target())),
                                menu_item("删除", true, MenuAction::DeleteRemote(target())),
                                separator(),
                                menu_item("复制路径", true, MenuAction::CopyPath(target().path)),
                                menu_item(
                                    "属性",
                                    true,
                                    MenuAction::Properties(remote_properties(
                                        &target(),
                                        Self::perms_string(entry.perms),
                                    )),
                                ),
                            ],
                        );
                    });
                }
            });
    }
    /// Applies a context-menu choice. Runs outside the row loops, where `&mut self`
    /// is free of the borrows the panes hold.
    fn dispatch(&mut self, action: MenuAction, ctx: &egui::Context) {
        match action {
            MenuAction::OpenLocal(path) => self.report(reveal(&path)),
            MenuAction::EditLocal(path) => self.report(edit_with(&path)),
            MenuAction::OpenRemote(target) => self.open_remote(target, false, ctx),
            MenuAction::EditRemote(target) => self.open_remote(target, true, ctx),
            MenuAction::Upload(path) => {
                let directory = path.is_dir();
                let remote = self.remote_child(&file_name_of(&path));
                self.transfer(path, remote, Direction::Upload, directory, ctx);
            }
            MenuAction::Download(target) => {
                if !remote::safe_local_name(&target.entry.name) {
                    self.error = Some("服务器返回了非法文件名".into());
                    return;
                }
                let directory = target.entry.directory;
                let local = PathBuf::from(&self.local_path).join(&target.entry.name);
                self.transfer(local, target.path, Direction::Download, directory, ctx);
            }
            MenuAction::RenameLocal(path) => {
                self.renaming = Some(Rename {
                    name: file_name_of(&path),
                    local: Some(path),
                    remote: None,
                });
            }
            MenuAction::RenameRemote(target) => {
                self.renaming = Some(Rename {
                    name: target.entry.name.clone(),
                    local: None,
                    remote: Some(target),
                });
            }
            MenuAction::DeleteLocal(path) => {
                if path.is_dir() && !dir_is_empty_locally(&path) {
                    self.confirm = Some(Confirm {
                        message: format!(
                            "目录 {} 非空，将连同其中所有内容一并删除。",
                            file_name_of(&path)
                        ),
                        action: ConfirmAction::DeleteLocal(path),
                    });
                } else {
                    self.delete_local(path);
                }
            }
            MenuAction::DeleteRemote(target) => {
                if !target.entry.directory {
                    let loaded = self.simple_operation();
                    spawn_delete_remote(self.connection.clone(), target, loaded, ctx);
                } else {
                    // Whether a directory is empty decides whether this needs
                    // confirming, and only the server can say.
                    let slot = self.simple_operation();
                    let connection = self.connection.clone();
                    let probe = target.clone();
                    let wake = wake(ctx);
                    remote::runtime().spawn(async move {
                        let result = async {
                            let sftp = connection.sftp("检查目录").await?;
                            remote::directory_is_empty(&sftp, &probe.path).await
                        }
                        .await;
                        *slot.lock().unwrap() = Some(match result {
                            Ok(true) => Ok(String::new()),
                            Ok(false) => Ok("非空".into()),
                            Err(e) => Err(format!("{e:#}")),
                        });
                        wake();
                    });
                    self.deleting = Some(target);
                }
            }
            MenuAction::CopyPath(text) => {
                ctx.copy_text(text);
                self.notice = Some("已复制路径".into());
            }
            MenuAction::Properties(properties) => self.properties = Some(properties),
        }
    }

    fn report(&mut self, result: Result<(), String>) {
        if let Err(e) = result {
            self.error = Some(e);
        }
    }

    /// Reserves the single operation slot for a one-off remote action, the same way
    /// `simple` does for the footer's mkdir/rename row.
    fn simple_operation(&mut self) -> Outcome {
        self.simple("文件操作")
    }

    /// Deletes a local file or tree on a worker thread. `remove_dir_all` over a
    /// large tree used to run inline and freeze the window for its duration.
    fn delete_local(&mut self, path: PathBuf) {
        let state = self.simple_operation();
        let wake = self.wake.clone();
        remote::runtime().spawn_blocking(move || {
            let outcome = if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            *state.lock().unwrap() = Some(match outcome {
                Ok(()) => Ok(format!("已删除 {}", file_name_of(&path))),
                Err(e) => Err(format!("删除失败：{e}")),
            });
            wake();
        });
    }

    fn delete_remote(&mut self, target: RemoteTarget, ctx: &egui::Context) {
        let loaded = self.simple_operation();
        spawn_delete_remote(self.connection.clone(), target, loaded, ctx);
    }

    /// Fetches a remote file into a scratch directory and opens it. In edit mode the
    /// copy is watched, and every save is sent back over the original.
    fn open_remote(&mut self, target: RemoteTarget, edit: bool, ctx: &egui::Context) {
        if target.entry.directory {
            self.error = Some("目录不能打开；双击进入即可".into());
            return;
        }
        if !remote::safe_local_name(&target.entry.name) {
            self.error = Some("服务器返回了非法文件名".into());
            return;
        }
        let Some(dir) = scratch_dir(self.edits.len()) else {
            self.error = Some("无法创建临时目录".into());
            return;
        };
        let local = dir.join(&target.entry.name);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        if edit {
            self.edits.push(EditSession {
                stop: stop.clone(),
                dir: dir.clone(),
            });
        }
        let connection = self.connection.clone();
        let remote = target.path.clone();
        let state = self.simple_operation();
        let wake = wake(ctx);
        remote::runtime().spawn(async move {
            let fail = |state: &Outcome, message: String| {
                *state.lock().unwrap() = Some(Err(message));
            };
            if let Err(e) = remote::fetch(&connection, &remote, &local).await {
                fail(&state, format!("{e:#}"));
                wake();
                return;
            }
            *state.lock().unwrap() = Some(Ok(if edit {
                "已打开，保存后自动回传".into()
            } else {
                "已下载并打开".into()
            }));
            wake();
            let opened = if edit {
                edit_with(&local)
            } else {
                reveal(&local)
            };
            if let Err(e) = opened {
                fail(&state, e);
                wake();
                return;
            }
            if !edit {
                return;
            }
            // Watch the scratch copy and push every save back. The staleness check
            // runs again after a settle delay so a half-written file is not sent.
            let mut last = save_stamp(&local);
            while !stop.load(std::sync::atomic::Ordering::Acquire) {
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                let now = save_stamp(&local);
                if now == last {
                    continue;
                }
                tokio::time::sleep(std::time::Duration::from_millis(700)).await;
                last = save_stamp(&local);
                let outcome = remote::put(&connection, &remote, &local).await;
                *state.lock().unwrap() = Some(match outcome {
                    Ok(()) => Ok("已回传".into()),
                    Err(e) => Err(format!("回传失败：{e:#}")),
                });
                wake();
            }
        });
    }

    /// Walks the tree to count what a recursive change would touch, so the
    /// confirmation can say how much it covers. The walk is the one the change
    /// itself would do, so the number is not an estimate.
    fn count_permissions(&mut self, properties: &mut Properties) {
        let Some(edit) = &properties.editable else {
            return;
        };
        let slot: Arc<Mutex<Option<Result<usize, String>>>> = Arc::new(Mutex::new(None));
        let state = slot.clone();
        let wake = self.wake.clone();
        match &edit.target {
            PermTarget::Local(path) => {
                let path = path.clone();
                std::thread::spawn(move || {
                    let result = walk_local(&path)
                        .map(|paths| paths.len())
                        .map_err(|e| format!("{e:#}"));
                    *state.lock().unwrap() = Some(result);
                    wake();
                });
            }
            PermTarget::Remote(target) => {
                let (connection, path) = (self.connection.clone(), target.path.clone());
                remote::runtime().spawn(async move {
                    let result: anyhow::Result<usize> = async {
                        let sftp = connection.sftp("统计目录").await?;
                        Ok(remote::walk_remote(&sftp, &path).await?.len())
                    }
                    .await;
                    *state.lock().unwrap() = Some(result.map_err(|e| format!("{e:#}")));
                    wake();
                });
            }
        }
        self.counting = Some(slot);
    }

    /// Picks up a finished count and puts it in front of the user.
    fn poll_count(&mut self) {
        let Some(slot) = &self.counting else {
            return;
        };
        let landed = slot.lock().unwrap().clone();
        let Some(landed) = landed else {
            return;
        };
        self.counting = None;
        let Some(properties) = &mut self.properties else {
            return;
        };
        let Some(edit) = &mut properties.editable else {
            return;
        };
        match landed {
            Ok(count) => edit.counted = Some(count),
            Err(e) => {
                self.error = Some(e);
                edit.counted = None;
            }
        }
    }

    /// Applies the mode, and the ownership if one was given.
    fn apply_permissions(&mut self, properties: &Properties) {
        let Some(edit) = &properties.editable else {
            return;
        };
        let (mode, recursive) = (edit.mode, edit.recursive);
        let (owner, group) = (edit.owner.clone(), edit.group.clone());
        let (progress, outcome) = self.simple_progress("修改权限");
        let wake = self.wake.clone();
        match &edit.target {
            PermTarget::Local(path) => {
                let path = path.clone();
                std::thread::spawn(move || {
                    let result = apply_local_mode(&path, mode, recursive, &progress, &wake);
                    *outcome.lock().unwrap() = Some(result.map_err(|e| format!("{e:#}")));
                    wake();
                });
            }
            PermTarget::Remote(target) => {
                let path = target.path.clone();
                let connection = self.connection.clone();
                remote::runtime().spawn(async move {
                    let result: anyhow::Result<String> = async {
                        let uid = match owner.trim() {
                            "" => None,
                            name => Some(remote::resolve_uid(&connection, name).await?),
                        };
                        let gid = match group.trim() {
                            "" => None,
                            name => Some(remote::resolve_gid(&connection, name).await?),
                        };
                        let sftp = connection.sftp("修改权限").await?;
                        let paths = if recursive {
                            remote::walk_remote(&sftp, &path).await?
                        } else {
                            vec![path.clone()]
                        };
                        let total = paths.len().max(1);
                        // Hoisted out of the loop: `||` cannot be mixed into a
                        // let-chain, and nesting the two conditions only to satisfy
                        // clippy would read worse than naming the question once.
                        let owner_wanted = uid.is_some() || gid.is_some();
                        // A refusal of the ownership half is recorded, not raised:
                        // the mode change is the part most servers allow, and losing
                        // it because the user is not root would be the wrong trade.
                        let mut refused = None;
                        for (index, target) in paths.iter().enumerate() {
                            remote::set_permissions(&sftp, target, mode).await?;
                            if owner_wanted
                                && let Err(e) = remote::set_owner(&sftp, target, uid, gid).await
                            {
                                refused.get_or_insert(format!("{e:#}"));
                            }
                            if let Ok(mut state) = progress.lock() {
                                state.done = index as u64 + 1;
                                state.total = total as u64;
                                state.message = "修改中".into();
                            }
                            wake();
                        }
                        Ok(match refused {
                            Some(e) => format!("权限已修改，所有者未修改：{e}"),
                            None if recursive => format!("已修改 {} 个条目", paths.len()),
                            None => "权限已修改".into(),
                        })
                    }
                    .await;
                    *outcome.lock().unwrap() = Some(result.map_err(|e| format!("{e:#}")));
                    wake();
                });
            }
        }
    }

    /// The 属性 / 确认删除 / 重命名 / 覆盖冲突 windows.
    fn dialogs(&mut self, ctx: &egui::Context, p: Palette) {
        self.show_conflict(ctx, p);
        // Taken out of `self` rather than cloned, so the permission editor can
        // write back what the user typed, and put back unless the window closed.
        if let Some(mut properties) = self.properties.take() {
            let mut open = true;
            let mut apply = false;
            let mut count = false;
            egui::Window::new("属性")
                .collapsible(false)
                .resizable(false)
                .default_width(460.0)
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .open(&mut open)
                .show(ctx, |ui| {
                    egui::Grid::new("properties")
                        .num_columns(2)
                        .spacing([12.0, 6.0])
                        .min_col_width(56.0)
                        // Cap the value column: without it the grid sizes to the
                        // longest value, so a deep path makes the window as wide as
                        // the path is long.
                        .max_col_width(320.0)
                        .show(ui, |ui| {
                            for (label, value) in [
                                ("名称", properties.name.clone()),
                                ("路径", properties.path.clone()),
                                ("类型", properties.kind.to_string()),
                                (
                                    "大小",
                                    properties.size.map_or_else(|| "—".to_string(), format_size),
                                ),
                                (
                                    "修改时间",
                                    properties.mtime.clone().unwrap_or_else(|| "—".into()),
                                ),
                            ] {
                                ui.label(hint(label, p));
                                // Wrapped rather than allowed to stretch the row.
                                ui.add(egui::Label::new(value).wrap());
                                ui.end_row();
                            }
                        });
                    if let Some(edit) = &mut properties.editable {
                        ui.separator();
                        let (asked_apply, asked_count) = permission_editor(ui, edit, p);
                        apply |= asked_apply;
                        count |= asked_count;
                    } else if let Some(perms) = &properties.perms {
                        ui.label(hint(&format!("权限：{perms}"), p));
                    }
                });
            // A recursive change is counted first, so the confirmation can say how
            // much it covers. The walk is the same one the change would do.
            if count {
                self.count_permissions(&mut properties);
            }
            if apply {
                self.apply_permissions(&properties);
            }
            if open {
                self.properties = Some(properties);
            }
        }

        if let Some(rename) = &mut self.renaming {
            let mut open = true;
            let mut submit = false;
            egui::Window::new("重命名")
                .collapsible(false)
                .resizable(false)
                .default_width(360.0)
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    let edit =
                        editing::field_with(ui, &mut rename.name, |edit| edit.desired_width(320.0));
                    edit.request_focus();
                    let valid = !rename.name.is_empty()
                        && !rename.name.contains(['/', '\\'])
                        && rename.name != "..";
                    if !valid {
                        ui.colored_label(p.danger, "名称不能为空，也不能包含斜杠或 ..");
                    }
                    ui.horizontal(|ui| {
                        if ui.add_enabled(valid, egui::Button::new("确定")).clicked()
                            || (valid
                                && edit.lost_focus()
                                && !editing::ime_composing(ui.ctx())
                                && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            submit = true;
                        }
                        if ui.button("取消").clicked() {
                            open = false;
                        }
                    });
                });
            if submit {
                let taken = self.renaming.take();
                if let Some(mut rename) = taken {
                    let name = std::mem::take(&mut rename.name);
                    if let Some(path) = rename.local {
                        rename_local(&path, &name, &mut self.error, &mut self.notice);
                    } else if let Some(target) = rename.remote {
                        let to = join_path(&parent_path(&target.path), &name);
                        let loaded = self.simple_operation();
                        spawn_rename_remote(self.connection.clone(), target, to, loaded, ctx);
                    }
                }
                self.refresh_local();
                self.refresh_remote(ctx);
            } else if !open {
                self.renaming = None;
            }
        }

        if let Some(confirm) = &self.confirm {
            let message = confirm.message.clone();
            let mut answer = None;
            egui::Window::new("确认删除")
                .collapsible(false)
                .resizable(false)
                .default_width(400.0)
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.add(egui::Label::new(RichText::new(message).color(p.warn)).wrap());
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("删除").clicked() {
                            answer = Some(true);
                        }
                        if ui.button("取消").clicked() {
                            answer = Some(false);
                        }
                    });
                });
            match answer {
                Some(true) => {
                    let action = self.confirm.take().map(|c| c.action);
                    match action {
                        Some(ConfirmAction::DeleteLocal(path)) => self.delete_local(path),
                        Some(ConfirmAction::DeleteRemote(target)) => {
                            self.delete_remote(target, ctx)
                        }
                        None => {}
                    }
                }
                Some(false) => self.confirm = None,
                None => {}
            }
        }

        // A directory delete waits here for the server to report whether the
        // directory holds anything, which decides if it needs confirming.
        if let Some(target) = self.deleting.clone() {
            let probe = self.operation.as_ref().and_then(Operation::outcome);
            match probe {
                Some(Ok(verdict)) => {
                    self.deleting = None;
                    self.operation = None;
                    if verdict.is_empty() {
                        self.delete_remote(target, ctx);
                    } else {
                        self.confirm = Some(Confirm {
                            message: format!(
                                "目录 {} 非空，将连同其中所有内容一并删除。",
                                target.entry.name
                            ),
                            action: ConfirmAction::DeleteRemote(target),
                        });
                    }
                }
                Some(Err(e)) => {
                    self.deleting = None;
                    self.operation = None;
                    self.error = Some(e);
                }
                None => {}
            }
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, open: &mut bool, p: Palette, hide_dotfiles: bool) {
        self.show_hidden = self.show_hidden || !hide_dotfiles;
        self.poll_question();
        self.poll_conflict();
        self.poll_batch(ctx);
        self.poll_transfers(ctx);
        self.poll_count();
        egui::Window::new(format!("文件 · {}", self.connection.profile.label()))
            .open(open)
            .default_size([900.0, 560.0])
            .min_width(560.0)
            .min_height(360.0)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("SFTP").color(p.accent));
                    if ui.checkbox(&mut self.show_hidden, "显示隐藏文件").changed() {
                        self.refresh_local();
                    }
                });
                let mut local_next = None;
                let mut remote_next = None;
                // Collected by the row loops and applied below, where `&mut self`
                // is free of the borrows the panes hold.
                let mut menu = None;
                let available = ui.available_width();
                let area = ui.available_rect_before_wrap();
                // The panes take whatever the footer left last frame — measured,
                // not guessed. Without this the content would be taller than the
                // window, and egui's `Resize` refuses to shrink a window below its
                // content (`resize.rs`: `desired_size.max(last_content_size)`), so
                // dragging the window shorter would spring straight back.
                let panes = egui::Rect::from_min_max(
                    area.min,
                    egui::pos2(
                        area.right(),
                        (area.bottom() - self.footer_height).max(area.top() + 80.0),
                    ),
                );
                // Side by side needs room for two readable columns; a narrow
                // window stacks them instead so it stays usable when shrunk.
                let stacked = available < 520.0;
                let pane = egui::Layout::top_down(egui::Align::LEFT);
                let (local_rect, remote_rect, divider) = if stacked {
                    let half = (panes.height() * self.split_ratio.clamp(0.2, 0.8)).round();
                    let cut = panes.top() + half;
                    (
                        egui::Rect::from_min_max(panes.min, egui::pos2(panes.right(), cut - 3.0)),
                        egui::Rect::from_min_max(egui::pos2(panes.left(), cut + 3.0), panes.max),
                        egui::Rect::from_min_max(
                            egui::pos2(panes.left(), cut - 3.0),
                            egui::pos2(panes.right(), cut + 3.0),
                        ),
                    )
                } else {
                    let left =
                        panes.left() + (available * self.split_ratio.clamp(0.25, 0.75)).round();
                    // A gap rather than a hairline butt-joint: with nothing drawn
                    // between them the two tables read as one continuous band of
                    // text, and the remote names look like they are sitting on the
                    // local timestamps.
                    const GAP: f32 = 7.0;
                    (
                        egui::Rect::from_min_max(panes.min, egui::pos2(left - GAP, panes.bottom())),
                        egui::Rect::from_min_max(egui::pos2(left + GAP, panes.top()), panes.max),
                        egui::Rect::from_min_max(
                            egui::pos2(left - 3.0, panes.top()),
                            egui::pos2(left + 3.0, panes.bottom()),
                        ),
                    )
                };
                let mut local_ui =
                    ui.new_child(egui::UiBuilder::new().max_rect(local_rect).layout(pane));
                self.show_local_pane(&mut local_ui, p, &mut local_next, &mut menu);
                let mut remote_ui =
                    ui.new_child(egui::UiBuilder::new().max_rect(remote_rect).layout(pane));
                self.show_remote_pane(&mut remote_ui, p, &mut remote_next, &mut menu);
                let drag = ui.interact(divider, ui.id().with("files-split"), egui::Sense::drag());
                if drag.dragged()
                    && let Some(pos) = drag.interact_pointer_pos()
                {
                    self.split_ratio = if stacked {
                        ((pos.y - panes.top()) / panes.height().max(1.0)).clamp(0.2, 0.8)
                    } else {
                        ((pos.x - panes.left()) / available.max(1.0)).clamp(0.25, 0.75)
                    };
                }
                drag.on_hover_cursor(if stacked {
                    egui::CursorIcon::ResizeVertical
                } else {
                    egui::CursorIcon::ResizeHorizontal
                });
                // A visible seam. Painted after the panes so it lands on top.
                let seam = egui::Stroke::new(1.0_f32, p.line);
                if stacked {
                    ui.painter()
                        .hline(divider.x_range(), divider.center().y, seam);
                } else {
                    ui.painter()
                        .vline(divider.center().x, divider.y_range(), seam);
                }
                ui.allocate_rect(panes, egui::Sense::hover());
                if let Some(path) = local_next {
                    self.local_path = path;
                    self.refresh_local();
                }
                if let Some(path) = remote_next {
                    self.remote_path = path;
                    self.refresh_remote(ctx);
                }
                if let Some(action) = menu {
                    self.dispatch(action, ctx);
                }
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    // Plain labels rather than painted glyphs: the arrow already
                    // says which way the file goes, and at this size a drawn icon
                    // competes with the text instead of helping it.
                    if ui
                        .add(egui::Button::new("上传 →").min_size(egui::vec2(80.0, 0.0)))
                        .clicked()
                        && let Some(path) = self.selected_local.clone()
                    {
                        let directory = path.is_dir();
                        let remote = self.remote_child(&file_name_of(&path));
                        self.transfer(path, remote, Direction::Upload, directory, ctx);
                    }
                    if ui
                        .add(egui::Button::new("← 下载").min_size(egui::vec2(80.0, 0.0)))
                        .clicked()
                        && let Some(e) = self.selected_remote.clone()
                    {
                        if !remote::safe_local_name(&e.name) {
                            self.error = Some("服务器返回了非法文件名".into());
                        } else {
                            let directory = e.directory;
                            self.transfer(
                                PathBuf::from(&self.local_path).join(&e.name),
                                self.remote_child(&e.name),
                                Direction::Download,
                                directory,
                                ctx,
                            );
                        }
                    }
                    ui.separator();
                    editing::field_with(ui, &mut self.name, |edit| edit.desired_width(160.0));
                    let mut op = None;
                    if ui
                        .add(egui::Button::new("建目录").min_size(egui::vec2(80.0, 0.0)))
                        .clicked()
                    {
                        op = Some(0);
                    }
                    if ui
                        .add(egui::Button::new("重命名").min_size(egui::vec2(80.0, 0.0)))
                        .clicked()
                    {
                        op = Some(1);
                    }
                    if let Some(op) = op {
                        if self.name.is_empty()
                            || self.name.contains(['/', '\\'])
                            || self.name == ".."
                        {
                            self.error = Some("请输入有效文件名".into());
                        } else {
                            let connection = self.connection.clone();
                            let target = self.remote_child(&self.name);
                            let source = self
                                .selected_remote
                                .as_ref()
                                .map(|e| self.remote_child(&e.name));
                            let state = self.simple("新建目录 / 重命名");
                            let wake = wake(ctx);
                            remote::runtime().spawn(async move {
                                let result: anyhow::Result<()> = async {
                                    let sftp = connection.sftp("新建目录或重命名").await?;
                                    if op == 0 {
                                        sftp.create_dir(target).await?;
                                    } else if let Some(source) = source {
                                        sftp.rename(source, target).await?;
                                    } else {
                                        anyhow::bail!("请先选择远程文件");
                                    }
                                    Ok(())
                                }
                                .await;
                                let result = result
                                    .map(|_| "操作完成，请刷新".to_string())
                                    .map_err(|e| e.to_string());
                                *state.lock().unwrap() = Some(result);
                                wake();
                            });
                        }
                    }
                });
                if let Some(operation) = &self.operation {
                    let state = operation.state();
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(state.name.clone()).color(p.accent));
                        if state.total > 0 {
                            ui.add(
                                egui::ProgressBar::new(state.fraction())
                                    .desired_width(160.0)
                                    .show_percentage(),
                            );
                            ui.label(hint(
                                &format!(
                                    "{} / {} · {}",
                                    format_size(state.done),
                                    format_size(state.total),
                                    state.message
                                ),
                                p,
                            ));
                        } else {
                            ui.label(hint(&state.message, p));
                        }
                        if operation.running() && ui.small_button("取消").clicked() {
                            operation
                                .cancel
                                .store(true, std::sync::atomic::Ordering::Release);
                        }
                    });
                    match operation.outcome() {
                        Some(Ok(message)) => {
                            ui.colored_label(p.accent, message);
                        }
                        Some(Err(e)) => {
                            ui.colored_label(p.danger, e);
                        }
                        None => {}
                    }
                }
                if let Some(error) = &self.error {
                    ui.colored_label(p.danger, error);
                }
                if let Some(notice) = self.notice.clone() {
                    ui.horizontal(|ui| {
                        ui.colored_label(p.accent, &notice);
                        if ui.small_button("×").clicked() {
                            self.notice = None;
                        }
                    });
                }
                ui.label("传输队列");
                egui::ScrollArea::vertical()
                    .id_salt("transfers")
                    .max_height(150.0)
                    .show(ui, |ui| {
                        for batch in &self.batches {
                            let members: Vec<&Transfer> = self
                                .transfers
                                .iter()
                                .filter(|t| t.batch == batch.id)
                                .collect();
                            let Some(first) = members.first() else {
                                continue;
                            };
                            if !batch.directory {
                                transfer_row(ui, first, p, ctx, false);
                                continue;
                            }
                            // A directory batch shows its own aggregate, then only
                            // the file actually moving. Queued and finished files
                            // are deliberately absent: a long tree would otherwise
                            // bury the live one.
                            let planned: u64 = members.iter().map(|t| t.planned).sum();
                            let settled: u64 = members
                                .iter()
                                .map(|t| {
                                    let s = t.state.lock().unwrap();
                                    // A settled entry counts its planned bytes,
                                    // whether it moved them or was skipped, so the
                                    // bar reads "how much of this batch is done
                                    // with" and still reaches the end.
                                    if s.finished { t.planned } else { s.done }
                                })
                                .sum();
                            let finished = members
                                .iter()
                                .filter(|t| t.state.lock().unwrap().finished)
                                .count();
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(format!(
                                    "{} {}/",
                                    direction_glyph(batch_direction(&members)),
                                    batch.name
                                )));
                                ui.add(
                                    egui::ProgressBar::new(if planned == 0 {
                                        0.0
                                    } else {
                                        settled as f32 / planned as f32
                                    })
                                    .desired_width(160.0)
                                    .show_percentage(),
                                );
                                ui.label(hint(
                                    &format!(
                                        "{} / {} · {}/{} 个文件",
                                        format_size(settled),
                                        format_size(planned),
                                        finished,
                                        members.len()
                                    ),
                                    p,
                                ));
                            });
                            if let Some(active) = members
                                .iter()
                                .find(|t| t.active.load(std::sync::atomic::Ordering::Acquire))
                            {
                                ui.indent(batch.id, |ui| {
                                    transfer_row(ui, active, p, ctx, true);
                                });
                            }
                        }
                    });
                // Measured, not guessed. The panes above reserved exactly
                // `area − footer_height`, so the two together add up to the
                // window's own content box and `Resize` is free to shrink it.
                self.footer_height = (ui.min_rect().bottom() - panes.bottom()).max(0.0);
            });
        // Outside the window closure, where `self` is free to move again.
        self.dialogs(ctx, p);
    }
}
/// The body of the overwrite conflict dialog.
///
/// A header, the file it is about, the sizes, then the choices — laid out like
/// a dialog rather than a list of sentences, because this one blocks a transfer
/// until it is answered.
pub(crate) fn conflict_body(
    ui: &mut egui::Ui,
    name: &str,
    existing: u64,
    incoming: u64,
    batch_size: usize,
    p: Palette,
) -> Option<remote::ConflictChoice> {
    let mut answer = None;
    ui.set_width(470.0);
    // A tinted header band, so the dialog announces itself before anything else
    // is read — the same size for every line is what made it look like a list.
    egui::Frame::new()
        .fill(p.warn.gamma_multiply(0.18))
        .inner_margin(egui::Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("目标已存在")
                    .color(p.warn)
                    .size(17.0)
                    .strong(),
            );
        });
    ui.add_space(12.0);
    ui.label(RichText::new(name).color(p.text).strong());
    ui.add_space(4.0);
    ui.label(
        RichText::new(format!(
            "已有 {} · 本次 {} · 本批共 {} 个文件",
            format_size(existing),
            format_size(incoming),
            batch_size
        ))
        .color(p.muted),
    );
    ui.add_space(14.0);
    // The 全部 buttons only mean something when the batch holds more than the
    // file that asked.
    let more = batch_size > 1;
    ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
    // The choices are grouped by what they affect: this file, the whole batch,
    // then the one that stops everything.
    ui.horizontal(|ui| {
        if ui.button("覆盖").clicked() {
            answer = Some(remote::ConflictChoice::Overwrite);
        }
        if ui.button("自动重命名").clicked() {
            answer = Some(remote::ConflictChoice::Rename);
        }
        if ui.button("跳过这个文件").clicked() {
            answer = Some(remote::ConflictChoice::Skip);
        }
    });
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(more, egui::Button::new("本批全部覆盖"))
            .clicked()
        {
            answer = Some(remote::ConflictChoice::OverwriteAll);
        }
        if ui
            .add_enabled(more, egui::Button::new("本批全部重命名"))
            .clicked()
        {
            answer = Some(remote::ConflictChoice::RenameAll);
        }
        if ui
            .add_enabled(more, egui::Button::new("本批全部跳过"))
            .clicked()
        {
            answer = Some(remote::ConflictChoice::SkipAll);
        }
    });
    ui.add_space(14.0);
    ui.separator();
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("取消剩余任务").clicked() {
            answer = Some(remote::ConflictChoice::CancelRemaining);
        }
        ui.label(hint("文件不会被改动", p));
    });
    answer
}

pub fn format_size(n: u64) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MiB", n as f64 / 1048576.0)
    } else if n >= 1024 {
        format!("{:.1} KiB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

/// One cell of a file-table row.
struct Cell {
    text: String,
    /// Right-align the metadata columns so their digits line up down the table.
    right: bool,
    color: egui::Color32,
    /// Shown on hover when `text` is a shortened form of something longer.
    full: Option<String>,
    /// A glyph drawn before the text. The text's width budget makes room for it,
    /// so an icon can never push a name into the next column.
    icon: Option<crate::icons::Icon>,
    /// Draw the shortcut badge over the icon.
    link: bool,
}

/// Pixel widths for the metadata columns. The name takes whatever is left, rather
/// than every column taking a fraction: fractions squeeze the metadata into
/// truncation on a narrow pane even when the names would have fitted.
struct Columns {
    size: f32,
    /// Zero when the table has no permissions to show: a column of width zero is
    /// dropped by [`column_offsets`].
    perms: f32,
    time: f32,
}

/// The local table carries permissions only where they exist. On Windows the
/// column's width is zero, so it drops out of the layout instead of showing a
/// fabricated mode.
#[cfg(unix)]
const LOCAL_COLUMNS: Columns = Columns {
    size: 68.0,
    perms: 88.0,
    time: 96.0,
};
#[cfg(not(unix))]
const LOCAL_COLUMNS: Columns = Columns {
    size: 68.0,
    perms: 0.0,
    time: 96.0,
};
const REMOTE_COLUMNS: Columns = Columns {
    size: 68.0,
    perms: 88.0,
    time: 96.0,
};

#[cfg(unix)]
const LOCAL_HEADERS: [&str; 4] = ["名称", "大小", "权限", "修改时间"];
#[cfg(not(unix))]
const LOCAL_HEADERS: [&str; 3] = ["名称", "大小", "修改时间"];

/// Space held between columns, belonging to neither of them.
const COLUMN_GAP: f32 = 10.0;

/// Left edge of every column, then the row's right edge.
///
/// The gap between columns is carried in these offsets but handed back by
/// [`column_rect`], so it ends up as space *between* the cells. Widening a cell
/// instead would not help: the text is free to fill whatever the cell is given.
fn column_offsets(columns: &Columns, width: f32) -> Vec<f32> {
    /// Below this the names stop being readable, and a clipped name tells you far
    /// less than a clipped size or timestamp does.
    const MIN_NAME: f32 = 72.0;
    let mut trailing: Vec<f32> = [columns.size, columns.perms, columns.time]
        .into_iter()
        .filter(|width| *width > 0.0)
        .collect();
    let count = trailing.len();
    let gaps = COLUMN_GAP * count as f32;
    let total: f32 = trailing.iter().sum::<f32>() + gaps;
    let available = (width - MIN_NAME).max(1.0);
    if total > available && total > gaps {
        // Squeeze the metadata instead of the names.
        let scale = (available - gaps).max(1.0) / (total - gaps);
        for column in &mut trailing {
            *column *= scale;
        }
    }
    let name = (width - trailing.iter().sum::<f32>() - gaps).max(0.0);
    let mut offsets = vec![0.0, name + COLUMN_GAP];
    let mut x = name + COLUMN_GAP;
    for (index, column) in trailing.iter().enumerate() {
        x += column;
        // The last column ends the row; every other one hands its gap back.
        if index + 1 < count {
            x += COLUMN_GAP;
        }
        offsets.push(x);
    }
    offsets
}

/// The rect of column `index`, or None when the table has fewer columns.
fn column_rect(row: egui::Rect, offsets: &[f32], index: usize) -> Option<egui::Rect> {
    let left = row.left() + *offsets.get(index)?;
    let right = row.left() + *offsets.get(index + 1)?;
    // Every column but the last gives the separator gap back, so it lands between
    // the cells rather than inside them.
    let right = if index + 2 < offsets.len() {
        right - COLUMN_GAP
    } else {
        right
    };
    Some(egui::Rect::from_min_max(
        egui::pos2(left, row.top()),
        egui::pos2(right, row.bottom()),
    ))
}

/// The narrowest a table may get before it scrolls sideways instead of squeezing
/// its columns. Below this the columns would clip, which tells the user less than
/// a scrollbar does.
const MIN_TABLE_WIDTH: f32 = 420.0;

/// The least a path field may be squeezed to before its shortcut icons are given
/// up instead. A field narrower than this cannot show a path, and forcing the
/// width anyway would push the row outside its pane.
const MIN_FIELD_WIDTH: f32 = 90.0;

/// Draws one file-table row at `width`. Returns the response and the rect of every
/// column, so the geometry is observable rather than implicit in the painting.
fn table_row(
    ui: &mut egui::Ui,
    cells: &[Cell],
    columns: &Columns,
    selected: bool,
    width: f32,
) -> (egui::Response, Vec<egui::Rect>) {
    const ROW_HEIGHT: f32 = 18.0;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::click());
    let offsets = column_offsets(columns, rect.width());
    let rects: Vec<_> = (0..cells.len())
        .filter_map(|index| column_rect(rect, &offsets, index))
        .collect();
    if ui.is_rect_visible(rect) {
        if selected || response.hovered() {
            let visuals = ui.style().interact_selectable(&response, selected);
            ui.painter().rect_filled(rect, 0, visuals.bg_fill);
        }
        let font =
            egui::FontId::proportional(ui.style().text_styles[&egui::TextStyle::Button].size);
        // A long name would otherwise paint straight across the columns to its
        // right: the painter is clipped to the scroll area, not to the cell.
        let mut hint: Option<String> = None;
        for (cell, cell_rect) in cells.iter().zip(&rects) {
            const PAD: f32 = 4.0;
            /// Between a leading icon and the text it introduces.
            const ICON_GAP: f32 = 5.0;
            let mut text_left = cell_rect.left() + PAD;
            let mut budget = cell_rect.width() - PAD * 2.0;
            if let Some(icon) = cell.icon {
                // Square, and inset from the row so it does not touch the text
                // above or below.
                let side = (cell_rect.height() - 4.0).max(8.0);
                let icon_rect = egui::Rect::from_min_size(
                    egui::pos2(text_left, cell_rect.center().y - side * 0.5),
                    egui::Vec2::splat(side),
                );
                crate::icons::draw(
                    ui.painter(),
                    icon_rect,
                    icon,
                    cell.color,
                    crate::icons::Size::Row.stroke(),
                );
                if cell.link {
                    crate::icons::link_badge(
                        ui.painter(),
                        icon_rect,
                        cell.color,
                        crate::icons::Size::Row.stroke(),
                    );
                }
                // Taken out of the text's budget, not added to the cell's, so a
                // name can never spill into the column beside it.
                text_left += side + ICON_GAP;
                budget -= side + ICON_GAP;
            }
            let (text, truncated) =
                truncate_to_width(ui, &cell.text, &font, cell.color, budget.max(0.0));
            // A cell that had to be cut offers its own full text.
            if truncated && hint.is_none() {
                hint = Some(cell.text.clone());
            }
            let (pos, align) = if cell.right {
                (
                    egui::pos2(cell_rect.right() - PAD, cell_rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                )
            } else {
                (
                    egui::pos2(text_left, cell_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                )
            };
            ui.painter()
                .text(pos, align, &text, font.clone(), cell.color);
        }
        // A shortened cell carries the full value for the tooltip even when it was
        // not itself cut — that is how the timestamp column works.
        if hint.is_none() {
            hint = cells.iter().find_map(|cell| cell.full.clone());
        }
        if let Some(full) = hint {
            return (response.on_hover_text(full), rects);
        }
    }
    (response, rects)
}

/// Trims `text` until it fits `max_width`, appending an ellipsis when it had to
/// cut. Splits on character boundaries, so multi-byte names survive intact.
fn truncate_to_width(
    ui: &egui::Ui,
    text: &str,
    font: &egui::FontId,
    color: egui::Color32,
    max_width: f32,
) -> (String, bool) {
    let measure = |candidate: &str| {
        ui.painter()
            .layout_no_wrap(candidate.to_owned(), font.clone(), color)
            .rect
            .width()
    };
    if max_width <= 0.0 {
        return (String::new(), !text.is_empty());
    }
    if measure(text) <= max_width {
        return (text.to_owned(), false);
    }
    let budget = max_width - measure("…");
    if budget <= 0.0 {
        return ("…".to_owned(), true);
    }
    // Largest character prefix that still fits once the ellipsis is appended.
    // The midpoint must be taken of the remaining range: `low + high.div_ceil(2)`
    // can jump past `high` and leave the range unshrunk, spinning forever.
    let (mut low, mut high) = (0usize, text.chars().count());
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        let candidate: String = text.chars().take(mid).collect();
        if measure(&candidate) <= budget {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    let mut truncated: String = text.chars().take(low).collect();
    truncated.push('…');
    (truncated, true)
}

/// The header row, laid out with the same columns as the rows below it.
fn table_header(ui: &mut egui::Ui, p: Palette, columns: &Columns, labels: &[&str], width: f32) {
    let cells: Vec<Cell> = labels
        .iter()
        .enumerate()
        .map(|(index, text)| Cell {
            text: (*text).to_string(),
            // Only the name column is left-aligned.
            right: index > 0,
            color: p.muted,
            full: None,
            icon: None,
            link: false,
        })
        .collect();
    table_row(ui, &cells, columns, false, width);
    ui.separator();
}

/// A checkbox for one permission bit.
fn bit_box(ui: &mut egui::Ui, mode: &mut u32, label: &str, mask: u32) {
    let mut on = *mode & mask != 0;
    if ui.checkbox(&mut on, label).changed() {
        if on {
            *mode |= mask;
        } else {
            *mode &= !mask;
        }
    }
}

/// The permission editor, drawn inside the 属性 window.
///
/// Returns `(apply, count)`: `count` asks for a recursive walk first, `apply`
/// asks for the change to go ahead. A recursive change always counts first, so the
/// confirmation can say how much it covers.
fn permission_editor(ui: &mut egui::Ui, edit: &mut PermissionEdit, p: Palette) -> (bool, bool) {
    let mut apply = false;
    let mut count = false;

    egui::Grid::new("permission-bits")
        .num_columns(4)
        .spacing([12.0, 4.0])
        .min_col_width(40.0)
        .show(ui, |ui| {
            ui.label(hint("", p));
            for heading in ["读", "写", "执行"] {
                ui.label(hint(heading, p));
            }
            ui.end_row();
            for (row, (who, read, write, execute)) in [
                ("所有者", 0o400, 0o200, 0o100),
                ("用户组", 0o040, 0o020, 0o010),
                ("其他", 0o004, 0o002, 0o001),
            ]
            .into_iter()
            .enumerate()
            {
                ui.label(hint(who, p));
                for (bit, mask) in [("读", read), ("写", write), ("执行", execute)] {
                    // Scoped, so three boxes labelled 读 in one grid stay distinct.
                    ui.push_id((row, bit), |ui| {
                        bit_box(ui, &mut edit.mode, bit, mask);
                    });
                }
                ui.end_row();
            }
        });
    ui.horizontal(|ui| {
        bit_box(ui, &mut edit.mode, "setuid", 0o4000);
        bit_box(ui, &mut edit.mode, "setgid", 0o2000);
        bit_box(ui, &mut edit.mode, "sticky", 0o1000);
    });
    // Shown live, because the checkboxes and the number are the same fact told two
    // ways and seeing both is how you catch a mis-click.
    ui.label(hint(&format!("八进制：{:04o}", edit.mode), p));

    // Ownership is an SFTP-side change: locally it would need root, and the
    // names are resolved against the server for a remote path.
    if !matches!(edit.target, PermTarget::Local(_)) {
        ui.horizontal(|ui| {
            ui.label(hint("所有者", p));
            editing::field_with(ui, &mut edit.owner, |field| {
                field.desired_width(96.0).hint_text("uid 或用户名")
            });
            ui.label(hint("用户组", p));
            editing::field_with(ui, &mut edit.group, |field| {
                field.desired_width(96.0).hint_text("gid 或组名")
            });
        });
    }
    ui.checkbox(&mut edit.recursive, "递归应用到该目录下所有内容");

    match edit.counted {
        Some(count) => {
            ui.colored_label(p.warn, format!("将修改 {count} 个条目，确认吗？"));
            ui.horizontal(|ui| {
                if ui.button("确认修改").clicked() {
                    apply = true;
                }
                if ui.button("取消").clicked() {
                    edit.counted = None;
                }
            });
        }
        None => {
            if ui.button("应用权限").clicked() {
                if edit.recursive {
                    count = true;
                } else {
                    apply = true;
                }
            }
        }
    }
    (apply, count)
}

/// `ls -F` suffixes: `/` for directories, `@` for symlinks.
fn entry_suffix(directory: bool, symlink: bool) -> &'static str {
    if directory {
        "/"
    } else if symlink {
        "@"
    } else {
        ""
    }
}

/// Which glyph stands for a row.
///
/// A name's extension decides; a directory or an executable overrides it, because
/// those are what the entry *is* rather than what it is called. An unknown
/// extension is a plain page rather than a guess.
fn file_icon(name: &str, directory: bool, executable: bool) -> crate::icons::Icon {
    use crate::icons::Icon;
    if directory {
        return Icon::Folder;
    }
    if executable {
        return Icon::FileBinary;
    }
    let extension = file_extension(name);
    match extension.as_str() {
        // Source, and the files that configure it.
        "rs" | "go" | "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "py" | "pyi" | "js"
        | "mjs" | "cjs" | "jsx" | "ts" | "tsx" | "java" | "kt" | "kts" | "rb" | "php" | "sh"
        | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd" | "lua" | "sql" | "html" | "htm"
        | "css" | "scss" | "sass" | "less" | "vue" | "svelte" | "swift" | "scala" | "hs" | "ml"
        | "ex" | "exs" | "erl" | "clj" | "dart" | "pl" | "vim" | "el" | "lisp" | "scm" | "asm"
        | "groovy" | "gradle" | "cmake" | "mk" | "json" | "yaml" | "yml" | "toml" | "ini"
        | "cfg" | "conf" | "xml" => Icon::FileCode,
        // Documents.
        "txt" | "md" | "markdown" | "rst" | "log" | "tex" | "rtf" | "doc" | "docx" | "odt"
        | "pdf" => Icon::FileText,
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "svg" | "webp" | "ico" | "tif" | "tiff"
        | "psd" | "xcf" | "raw" | "heic" | "avif" => Icon::FileImage,
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "7z" | "rar" | "lz" | "lz4"
        | "deb" | "rpm" | "apk" | "jar" | "war" | "iso" | "cab" | "msix" | "nupkg" => {
            Icon::FileArchive
        }
        // Audio and video. `.ts` is not here: in a shell it is far more often
        // TypeScript than an MPEG transport stream.
        "mp3" | "wav" | "flac" | "ogg" | "oga" | "m4a" | "aac" | "opus" | "wma" | "mid"
        | "midi" | "mp4" | "mkv" | "avi" | "mov" | "webm" | "wmv" | "flv" | "m4v" | "mpg"
        | "mpeg" | "3gp" => Icon::FileMedia,
        // Things that are not meant to be read.
        "exe" | "dll" | "so" | "dylib" | "o" | "a" | "lib" | "obj" | "bin" | "com" | "msi"
        | "app" | "elf" | "ko" | "sys" | "wasm" => Icon::FileBinary,
        _ => Icon::File,
    }
}

/// The lowercased extension of a name, or empty when there is none.
///
/// A leading dot is part of the name, not an extension: `.bashrc` is a dotfile,
/// not a file of type "bashrc".
fn file_extension(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => ext.to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// Joins a name onto a POSIX directory path without doubling the root slash.
fn join_path(parent: &str, name: &str) -> String {
    format!("{}/{}", parent.trim_end_matches('/'), name)
}

/// The parent of a POSIX path, computed rather than appended, so the address bar
/// never ends up showing a `..` component.
fn parent_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) => "/".to_string(),
        Some(index) => trimmed[..index].to_string(),
        None if trimmed.is_empty() => "/".to_string(),
        None => trimmed.to_string(),
    }
}

/// A concrete height for a pane's list. In an auto-sizing container
/// `available_height` can be unbounded, which would let the list grow forever.
fn list_height(ui: &egui::Ui) -> f32 {
    let available = ui.available_height();
    if available.is_finite() {
        available.max(80.0)
    } else {
        280.0
    }
}

/// `YYYY-MM-DD HH:MM` in UTC from Unix seconds. Hand-rolled so a single format
/// string does not pull a date crate into the build and the licence manifest.
fn format_time(unix_seconds: u64) -> String {
    // Short enough for a narrow column, the way `ls -l` picks a form that fits:
    // the time of day for this year, the year for anything older. The full value
    // is one hover away.
    let (year, month, day, hour, minute) = civil_parts(unix_seconds);
    if year == current_year() {
        format!("{month:02}-{day:02} {hour:02}:{minute:02}")
    } else {
        format!("{year:04}-{month:02}-{day:02}")
    }
}

/// The timestamp in full, for the tooltip over a shortened one.
fn format_time_full(unix_seconds: u64) -> String {
    let (year, month, day, hour, minute) = civil_parts(unix_seconds);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

fn civil_parts(unix_seconds: u64) -> (i64, u32, u32, u64, u64) {
    let (year, month, day) = civil_from_days((unix_seconds / 86_400) as i64);
    let seconds = unix_seconds % 86_400;
    (year, month, day, seconds / 3600, (seconds % 3600) / 60)
}

/// The year a timestamp is compared against when choosing a short form. Taken
/// from the clock so a file written this year reads as recent.
fn current_year() -> i64 {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let (year, ..) = civil_parts(seconds);
    year
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 to a civil date.
/// <https://howardhinnant.github.io/date_algorithms.html>
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Opens a path with whatever the OS associates with it.
fn reveal(path: &Path) -> Result<(), String> {
    open_command(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("无法打开 {}：{e}", path.display()))
}

/// The command that hands a path to the OS default handler. On Windows the empty
/// argument is required, otherwise `start` reads the path as a window title.
fn open_command(path: &Path) -> Command {
    let mut command = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else if cfg!(target_os = "macos") {
        Command::new("open")
    } else {
        Command::new("xdg-open")
    };
    command.arg(path);
    command
}

/// Opens a URL in the OS default browser.
pub(crate) fn open_url(url: &str) -> Result<(), String> {
    let mut command = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else if cfg!(target_os = "macos") {
        Command::new("open")
    } else {
        Command::new("xdg-open")
    };
    command
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("无法打开浏览器：{e}"))
}

/// Opens a path in a text editor rather than in its associated application.
fn edit_with(path: &Path) -> Result<(), String> {
    let Some(program) = editor_program() else {
        // Windows always has Notepad. macOS has no single editor path, so ask
        // the system for its default text editor rather than for whatever the
        // file type is associated with.
        #[cfg(target_os = "macos")]
        {
            return Command::new("open")
                .arg("-t")
                .arg(path)
                .spawn()
                .map(|_| ())
                .map_err(|e| format!("无法启动编辑器：{e}"));
        }
        #[cfg(not(target_os = "macos"))]
        return reveal(path);
    };
    Command::new(&program)
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("无法启动编辑器 {program}：{e}"))
}

/// The editor to use, from the environment. Split from the lookup so it is
/// testable without launching anything or depending on this machine's variables.
fn editor_program_from(lookup: impl Fn(&str) -> Option<String>) -> Option<String> {
    ["VISUAL", "EDITOR"]
        .into_iter()
        .find_map(&lookup)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn editor_program() -> Option<String> {
    editor_program_from(|name| std::env::var(name).ok()).or_else(|| {
        // Notepad is always present. Falling back to the file association would
        // open a .json in whatever edits JSON, which is not what 编辑 promises.
        cfg!(windows).then(|| "notepad.exe".to_string())
    })
}

/// A scratch directory for one editor round-trip. The index keeps two files that
/// share a name from colliding.
fn scratch_dir(index: usize) -> Option<PathBuf> {
    let dir = std::env::temp_dir()
        .join("gterminal-edit")
        .join(format!("{}-{index}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// What the editor round-trip watches: size plus modification time.
fn save_stamp(path: &Path) -> Option<(u64, u64)> {
    let metadata = std::fs::metadata(path).ok()?;
    let mtime = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some((metadata.len(), mtime))
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Whether a local directory holds anything, so the caller can decide whether an
/// irreversible delete needs confirming. An unreadable directory counts as
/// non-empty, which errs toward asking.
fn dir_is_empty_locally(path: &Path) -> bool {
    std::fs::read_dir(path)
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(false)
}

fn rename_local(path: &Path, name: &str, error: &mut Option<String>, notice: &mut Option<String>) {
    let Some(parent) = path.parent() else {
        *error = Some("无法确定所在目录".into());
        return;
    };
    let to = parent.join(name);
    if to.exists() {
        *error = Some("目标已存在，请换一个名字".into());
        return;
    }
    match std::fs::rename(path, &to) {
        Ok(()) => *notice = Some(format!("已重命名为 {name}")),
        Err(e) => *error = Some(format!("重命名失败：{e}")),
    }
}

fn local_properties(entry: &LocalEntry, path: &Path) -> Properties {
    // A symlink's own mode is not changeable in a useful way — `chmod` follows
    // the link — so it is shown but not offered for editing.
    let editable = (entry.perms.is_some() && !entry.symlink).then(|| PermissionEdit {
        target: PermTarget::Local(path.to_path_buf()),
        mode: entry.perms.unwrap_or(0o644) & 0o7777,
        owner: String::new(),
        group: String::new(),
        recursive: false,
        counted: None,
    });
    Properties {
        name: entry.name.clone(),
        path: path.display().to_string(),
        kind: if entry.symlink {
            "符号链接"
        } else if entry.directory {
            "目录"
        } else {
            "文件"
        },
        // A directory's own size says nothing useful about what is inside it.
        size: (!entry.directory).then_some(entry.size),
        mtime: entry.mtime.map(format_time),
        // Windows carries no POSIX mode, so the row is left out rather than faked.
        perms: entry.perms.map(|m| Files::perms_string(Some(m))),
        editable,
    }
}

/// Walks a local tree without following symlinks, yielding the root and then
/// everything under it. The counterpart to `remote::walk_remote`, so a recursive
/// permission change behaves the same on both sides.
fn walk_local(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut paths = vec![root.to_path_buf()];
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = std::fs::read_dir(&directory)
            .map_err(|e| anyhow::anyhow!("无法读取 {}：{e}", directory.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            // `symlink_metadata` does not follow the link, so a symlinked
            // directory is recorded but never descended.
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            paths.push(path.clone());
            if metadata.is_dir() {
                stack.push(path);
            }
        }
    }
    Ok(paths)
}

/// Applies `mode` to a local path, over its tree when `recursive`. Symlinks are
/// skipped so a recursive change never follows one onto its target.
#[cfg(unix)]
fn apply_local_mode(
    path: &Path,
    mode: u32,
    recursive: bool,
    progress: &Arc<Mutex<ztransfer::Progress>>,
    wake: &remote::Wake,
) -> anyhow::Result<String> {
    use std::os::unix::fs::PermissionsExt;
    let mut paths = if recursive {
        walk_local(path)?
    } else {
        vec![path.to_path_buf()]
    };
    // Deepest first: a directory whose new mode drops the execute bit can no
    // longer be entered, so its contents have to be changed before its own mode.
    paths.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    let total = paths.len().max(1);
    for (index, target) in paths.iter().enumerate() {
        let is_symlink = std::fs::symlink_metadata(target)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false);
        if !is_symlink {
            std::fs::set_permissions(target, std::fs::Permissions::from_mode(mode))
                .map_err(|e| anyhow::anyhow!("无法修改 {}：{e}", target.display()))?;
        }
        if let Ok(mut state) = progress.lock() {
            state.done = index as u64 + 1;
            state.total = total as u64;
            state.message = "修改中".into();
        }
        wake();
    }
    Ok(if recursive {
        format!("已修改 {} 个条目", paths.len())
    } else {
        "权限已修改".into()
    })
}

/// Windows has no POSIX mode to apply. The editor is never offered there, so
/// this is only reached if that ever changes.
#[cfg(not(unix))]
fn apply_local_mode(
    _path: &Path,
    _mode: u32,
    _recursive: bool,
    _progress: &Arc<Mutex<ztransfer::Progress>>,
    _wake: &remote::Wake,
) -> anyhow::Result<String> {
    anyhow::bail!("此平台不支持 POSIX 权限")
}

fn remote_properties(target: &RemoteTarget, perms: String) -> Properties {
    // Seeded from what the listing already reported, so the dialog opens on the
    // entry's real mode rather than a guess.
    let mode = target.entry.perms.unwrap_or(0o644);
    Properties {
        name: target.entry.name.clone(),
        path: target.path.clone(),
        kind: if target.entry.directory {
            "目录"
        } else {
            "文件"
        },
        size: (!target.entry.directory).then_some(target.entry.size),
        mtime: target.entry.mtime.map(|t| format_time(u64::from(t))),
        perms: Some(perms),
        editable: Some(PermissionEdit {
            target: PermTarget::Remote(target.clone()),
            mode,
            // SFTP carries ids, not names, so these start as numbers. A name typed
            // here is resolved on the server before anything is sent.
            owner: target
                .entry
                .uid
                .map(|id| id.to_string())
                .unwrap_or_default(),
            group: target
                .entry
                .gid
                .map(|id| id.to_string())
                .unwrap_or_default(),
            recursive: false,
            counted: None,
        }),
    }
}

fn spawn_delete_remote(
    connection: Arc<Connection>,
    target: RemoteTarget,
    state: Outcome,
    ctx: &egui::Context,
) {
    let wake = wake(ctx);
    let (path, is_dir) = (target.path, target.entry.directory);
    remote::runtime().spawn(async move {
        let result = async {
            let sftp = connection.sftp("删除").await?;
            remote::remove(&sftp, &path, is_dir).await
        }
        .await;
        *state.lock().unwrap() = Some(
            result
                .map(|_| "已删除".to_string())
                .map_err(|e| format!("{e:#}")),
        );
        wake();
    });
}

fn spawn_rename_remote(
    connection: Arc<Connection>,
    target: RemoteTarget,
    to: String,
    state: Outcome,
    ctx: &egui::Context,
) {
    let wake = wake(ctx);
    remote::runtime().spawn(async move {
        let result = async {
            let sftp = connection.sftp("重命名").await?;
            remote::rename(&sftp, &target.path, &to).await
        }
        .await;
        *state.lock().unwrap() = Some(
            result
                .map(|_| "已重命名".to_string())
                .map_err(|e| format!("{e:#}")),
        );
        wake();
    });
}

/// One row of a file-pane context menu. An entry that does not apply to the row is
/// greyed out, rather than left clickable and failing afterwards.
struct MenuItem {
    label: &'static str,
    enabled: bool,
    /// None marks a separator.
    action: Option<MenuAction>,
}

fn menu_item(label: &'static str, enabled: bool, action: MenuAction) -> MenuItem {
    MenuItem {
        label,
        enabled,
        action: Some(action),
    }
}

fn separator() -> MenuItem {
    MenuItem {
        label: "",
        enabled: false,
        action: None,
    }
}

fn context_menu(ui: &mut egui::Ui, menu: &mut Option<MenuAction>, items: Vec<MenuItem>) {
    for item in items {
        let Some(action) = item.action else {
            ui.separator();
            continue;
        };
        if ui
            .add_enabled(item.enabled, egui::Button::new(item.label))
            .clicked()
        {
            *menu = Some(action);
            ui.close();
        }
    }
}

fn direction_glyph(direction: Direction) -> &'static str {
    match direction {
        Direction::Upload => "↑",
        Direction::Download => "↓",
    }
}

/// The settled queue entries to drop, oldest first. Split from the queue itself so
/// it can be checked without a live connection to build a `Files` around.
fn settled_to_drop(settled: &[usize], keep: usize) -> &[usize] {
    &settled[..settled.len().saturating_sub(keep)]
}

/// Every entry of a batch moves the same way, so the first one speaks for all.
fn batch_direction(members: &[&Transfer]) -> Direction {
    members.first().map_or(Direction::Upload, |t| t.direction)
}

/// One file's queue row: name, progress, and whatever control applies right now.
fn transfer_row(
    ui: &mut egui::Ui,
    transfer: &Transfer,
    p: Palette,
    ctx: &egui::Context,
    nested: bool,
) {
    let s = transfer.state.lock().unwrap().clone();
    ui.horizontal(|ui| {
        let name = file_name_of(&transfer.local);
        // A nested row names just the file: the batch header above it already
        // carries the direction and the directory.
        if nested {
            ui.label(hint(&name, p));
        } else {
            ui.label(format!("{} {name}", direction_glyph(transfer.direction)));
        }
        ui.add(
            egui::ProgressBar::new(if s.total == 0 {
                0.0
            } else {
                s.done as f32 / s.total as f32
            })
            .desired_width(if nested { 120.0 } else { 160.0 }),
        );
        ui.label(hint(
            &format!(
                "{} / {} · {}",
                format_size(s.done),
                format_size(s.total),
                s.message
            ),
            p,
        ));
        if s.running && ui.small_button("暂停").clicked() {
            transfer
                .pause
                .store(true, std::sync::atomic::Ordering::Release);
        }
        if s.running && ui.small_button("取消").clicked() {
            transfer.cancel();
        }
        if !s.running && !s.finished && ui.small_button("继续").clicked() {
            transfer.start(wake(ctx));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_shorten_themselves_like_ls() {
        // `civil_parts` is the shared core, so it carries the calendar checks.
        assert_eq!(civil_parts(0), (1970, 1, 1, 0, 0));
        assert_eq!(civil_parts(951_782_400), (2000, 2, 29, 0, 0));
        assert_eq!(civil_parts(1_709_164_800), (2024, 2, 29, 0, 0));
        assert_eq!(civil_parts(1_709_251_200), (2024, 3, 1, 0, 0));
        assert_eq!(civil_parts(1_709_214_300), (2024, 2, 29, 13, 45));
        // Past 2038, which the 32-bit SFTP mtime field cannot itself express.
        assert_eq!(civil_parts(4_102_444_800), (2100, 1, 1, 0, 0));

        // The full form is what the tooltip shows, so it never varies.
        assert_eq!(format_time_full(0), "1970-01-01 00:00");
        assert_eq!(format_time_full(1_709_214_300), "2024-02-29 13:45");

        // The short form drops the clock for older files and the year for this
        // year's, so it is always shorter than the full one.
        let this_year = current_year();
        for seconds in [0, 951_782_400, 1_709_214_300, 4_102_444_800] {
            let (year, ..) = civil_parts(seconds);
            let short = format_time(seconds);
            let limit = if year == this_year { 11 } else { 10 };
            assert!(
                short.len() <= limit,
                "{short:?} is wider than the column is sized for"
            );
            assert!(short.len() < format_time_full(seconds).len());
        }
    }

    #[test]
    fn parent_path_never_leaves_a_dotdot_component() {
        assert_eq!(parent_path("/root/etc"), "/root");
        assert_eq!(parent_path("/root"), "/");
        assert_eq!(parent_path("/"), "/");
        assert_eq!(parent_path("/root/"), "/");
        assert_eq!(parent_path(""), "/");
    }

    #[test]
    fn join_path_does_not_double_the_root_slash() {
        assert_eq!(join_path("/root", "etc"), "/root/etc");
        assert_eq!(join_path("/", "etc"), "/etc");
        assert_eq!(join_path("/root/", "etc"), "/root/etc");
    }

    #[test]
    fn file_icons_follow_the_name() {
        use crate::icons::Icon;
        // What a thing *is* beats what it is called.
        assert_eq!(file_icon("src", true, false), Icon::Folder);
        assert_eq!(file_icon("archive.zip", true, false), Icon::Folder);
        assert_eq!(file_icon("run.sh", false, true), Icon::FileBinary);

        assert_eq!(file_icon("main.rs", false, false), Icon::FileCode);
        assert_eq!(file_icon("Makefile", false, false), Icon::File);
        assert_eq!(file_icon("config.toml", false, false), Icon::FileCode);
        assert_eq!(file_icon("notes.md", false, false), Icon::FileText);
        assert_eq!(file_icon("photo.PNG", false, false), Icon::FileImage);
        assert_eq!(file_icon("backup.tar.gz", false, false), Icon::FileArchive);
        assert_eq!(file_icon("clip.mp4", false, false), Icon::FileMedia);
        assert_eq!(file_icon("libfoo.so", false, false), Icon::FileBinary);
        // Unknown, and no extension at all, both fall back to a plain page.
        assert_eq!(file_icon("mystery.qqq", false, false), Icon::File);
        assert_eq!(file_icon("README", false, false), Icon::File);
    }

    /// A leading dot belongs to the name. `.bashrc` is a dotfile, not a file of
    /// type "bashrc" — treating it as one would put it in a category nothing else
    /// shares.
    #[test]
    fn a_dotfile_has_no_extension() {
        assert_eq!(file_extension(".bashrc"), "");
        assert_eq!(file_extension(".gitignore"), "");
        assert_eq!(file_extension("."), "");
        assert_eq!(file_extension(".."), "");
        assert_eq!(file_extension(""), "");
        assert_eq!(file_extension("archive.tar.gz"), "gz");
        assert_eq!(file_extension("main.RS"), "rs");
        // A dot that is part of the name, not before an extension.
        assert_eq!(file_extension("my file.txt"), "txt");
    }

    #[test]
    fn entry_suffix_follows_the_ls_convention() {
        assert_eq!(entry_suffix(true, false), "/");
        assert_eq!(entry_suffix(false, true), "@");
        assert_eq!(entry_suffix(false, false), "");
        assert_eq!(entry_suffix(true, true), "/");
    }

    #[test]
    fn columns_are_ordered_and_span_the_row() {
        let offsets = column_offsets(&REMOTE_COLUMNS, 600.0);
        assert_eq!(offsets.len(), 5, "four columns have five edges");
        assert_eq!(offsets[0], 0.0);
        for pair in offsets.windows(2) {
            assert!(pair[1] > pair[0], "edges must increase: {offsets:?}");
        }
        assert!((offsets[4] - 600.0).abs() < 0.01, "columns span the row");
    }

    #[test]
    fn local_columns_follow_the_platform_permissions() {
        let edges = column_offsets(&LOCAL_COLUMNS, 600.0).len();
        #[cfg(unix)]
        assert_eq!(edges, 5, "a POSIX local table has a permissions column");
        #[cfg(not(unix))]
        assert_eq!(edges, 4, "Windows has no permissions column");
    }

    /// Adjacent columns have to be separated by more than their cells' own
    /// padding, or a value one character wider than expected ends up touching its
    /// neighbour with nothing between them.
    #[test]
    fn columns_are_separated_by_a_gap() {
        for width in [320.0, 480.0, 700.0] {
            let offsets = column_offsets(&REMOTE_COLUMNS, width);
            let row = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 18.0));
            let rects: Vec<_> = (0..offsets.len() - 1)
                .filter_map(|index| column_rect(row, &offsets, index))
                .collect();
            for pair in rects.windows(2) {
                let gap = pair[1].left() - pair[0].right();
                assert!(
                    gap >= COLUMN_GAP - 0.01,
                    "columns sit {gap} apart at width {width}"
                );
            }
            // The gap is given back by every column but the last, so the row still
            // ends exactly at its right edge.
            assert!((rects.last().unwrap().right() - row.right()).abs() < 0.01);
        }
    }

    /// A pane's rows have to stay inside that pane. They are laid out in a child
    /// `Ui` nested inside a `ScrollArea`, and if either reported the window's width
    /// rather than the pane's, the remote table's first column would paint across
    /// into the local table on its left.
    #[test]
    fn a_panes_rows_stay_inside_the_pane() {
        const PANE_LEFT: f32 = 460.0;
        const PANE_RIGHT: f32 = 900.0;
        let ctx = egui::Context::default();
        let cells = [
            Cell {
                text: "readme.txt".into(),
                right: false,
                color: egui::Color32::WHITE,
                full: None,
                icon: None,
                link: false,
            },
            Cell {
                text: "1.0 KiB".into(),
                right: true,
                color: egui::Color32::GRAY,
                full: None,
                icon: None,
                link: false,
            },
            Cell {
                text: "-rw-r--r--".into(),
                right: true,
                color: egui::Color32::GRAY,
                full: None,
                icon: None,
                link: false,
            },
            Cell {
                text: "03-14 20:32".into(),
                right: true,
                color: egui::Color32::GRAY,
                full: None,
                icon: None,
                link: false,
            },
        ];
        let mut measured: Option<egui::Rect> = None;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 600.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    // Exactly the shape the file window builds for its panes.
                    let pane = egui::Rect::from_min_max(
                        egui::pos2(PANE_LEFT, 0.0),
                        egui::pos2(PANE_RIGHT, 400.0),
                    );
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(pane)
                            .layout(egui::Layout::top_down(egui::Align::LEFT)),
                    );
                    egui::ScrollArea::vertical()
                        .id_salt("test-pane")
                        .auto_shrink([false, false])
                        .max_height(300.0)
                        .show(&mut child, |ui| {
                            measured =
                                Some(table_row(ui, &cells, &REMOTE_COLUMNS, false, 440.0).0.rect);
                        });
                });
            },
        );
        let row = measured.expect("a row was laid out");
        assert!(
            row.left() >= PANE_LEFT - 0.5,
            "the row started left of its pane: {row:?}"
        );
        assert!(
            row.right() <= PANE_RIGHT + 0.5,
            "the row ran past its pane: {row:?}"
        );
    }

    /// Drives a real egui frame, so the row geometry is checked against the layout
    /// engine rather than against a reimplementation of it.
    #[test]
    fn table_rows_lay_cells_out_left_to_right_without_overlap() {
        let ctx = egui::Context::default();
        let cells = [
            Cell {
                text: "readme.txt".into(),
                right: false,
                color: egui::Color32::WHITE,
                full: None,
                icon: None,
                link: false,
            },
            Cell {
                text: "1.0 KiB".into(),
                right: true,
                color: egui::Color32::GRAY,
                full: None,
                icon: None,
                link: false,
            },
            Cell {
                text: "-rw-r--r--".into(),
                right: true,
                color: egui::Color32::GRAY,
                full: None,
                icon: None,
                link: false,
            },
            Cell {
                text: "2024-02-29 13:45".into(),
                right: true,
                color: egui::Color32::GRAY,
                full: None,
                icon: None,
                link: false,
            },
        ];
        let mut measured: Option<(egui::Rect, Vec<egui::Rect>)> = None;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 400.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let (response, rects) = table_row(ui, &cells, &REMOTE_COLUMNS, false, 440.0);
                    measured = Some((response.rect, rects));
                });
            },
        );
        let (row, rects) = measured.expect("the frame rendered a row");
        assert_eq!(rects.len(), cells.len());
        for pair in rects.windows(2) {
            assert!(
                pair[0].right() <= pair[1].left() + 0.01,
                "columns overlap: {:?} then {:?}",
                pair[0],
                pair[1]
            );
        }
        // The rightmost metadata column ends exactly at the row's right edge.
        assert!((rects[3].right() - row.right()).abs() < 0.01);
    }

    /// A name wider than its column used to paint straight across the columns to
    /// its right, because the painter is only clipped to the scroll area.
    #[test]
    fn truncated_cells_never_exceed_their_column() {
        let ctx = egui::Context::default();
        let font = egui::FontId::proportional(14.0);
        let color = egui::Color32::WHITE;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 400.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let measure = |text: &str| {
                        ui.painter()
                            .layout_no_wrap(text.to_owned(), font.clone(), color)
                            .rect
                            .width()
                    };
                    for (text, budget) in [
                        ("short.txt", 300.0),
                        ("a-really-long-file-name-that-cannot-fit.bin", 120.0),
                        ("超长的中文文件名需要被安全截断", 60.0),
                        ("", 100.0),
                        ("no-room.bin", 0.0),
                    ] {
                        let (out, truncated) = truncate_to_width(ui, text, &font, color, budget);
                        let width = measure(&out);
                        assert!(
                            width <= budget.max(0.0) + 0.5,
                            "{text:?} in {budget} produced {out:?}, {width} wide"
                        );
                        if truncated {
                            // A cut string is either marked, or had no room at all.
                            assert!(
                                out.is_empty() || out.ends_with('…'),
                                "{out:?} should be marked as cut"
                            );
                        } else {
                            assert_eq!(out, text, "text that fits must come back intact");
                        }
                    }
                    let (out, truncated) = truncate_to_width(ui, "fits.bin", &font, color, 300.0);
                    assert_eq!(out, "fits.bin");
                    assert!(!truncated);
                });
            },
        );
    }

    /// The editor lookup is the one part of 编辑 that can be pinned down without
    /// launching anything or depending on this machine's environment.
    #[test]
    fn editor_lookup_prefers_visual_over_editor() {
        fn env<'a>(
            pairs: &'a [(&'static str, &'static str)],
        ) -> impl Fn(&str) -> Option<String> + 'a {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_string())
            }
        }
        assert_eq!(
            editor_program_from(env(&[("EDITOR", "nano"), ("VISUAL", "code")])),
            Some("code".to_string())
        );
        assert_eq!(
            editor_program_from(env(&[("EDITOR", "nano")])),
            Some("nano".to_string())
        );
        assert_eq!(
            editor_program_from(env(&[("VISUAL", " vim ")])),
            Some("vim".to_string())
        );
        // A blank variable must not become an empty program name.
        assert_eq!(editor_program_from(env(&[("EDITOR", "   ")])), None);
        assert_eq!(editor_program_from(env(&[])), None);
    }

    /// The save watcher keys off size and mtime, so a rewrite that changes only
    /// one of them still registers.
    #[test]
    fn save_stamp_notices_a_rewrite() {
        let dir = scratch_dir(usize::MAX).expect("scratch directory");
        let file = dir.join("stamp.txt");
        std::fs::write(&file, b"one").unwrap();
        let first = save_stamp(&file);
        assert!(first.is_some());
        std::fs::write(&file, b"longer than before").unwrap();
        assert_ne!(
            save_stamp(&file),
            first,
            "a growing file must change the stamp"
        );
        assert_eq!(save_stamp(&dir.join("absent.txt")), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The queue keeps the newest settled entries and drops the rest, oldest
    /// first — removing them in that order is what leaves later indices valid.
    #[test]
    fn pruning_keeps_the_newest_settled_entries() {
        assert!(
            settled_to_drop(&[1, 2, 3], 5).is_empty(),
            "nothing should be dropped under the cap"
        );
        assert_eq!(settled_to_drop(&[1, 2, 3], 3), [] as [usize; 0]);
        // Oldest first, so the newest survive.
        assert_eq!(settled_to_drop(&[2, 5, 7, 9], 2), [2, 5]);
        assert_eq!(settled_to_drop(&[0, 1, 2, 3, 4], 1), [0, 1, 2, 3]);
    }

    /// Local POSIX modes are read from the listing, and a chmod round-trips
    /// through the same routine the 属性 editor uses.
    #[test]
    #[cfg(unix)]
    fn local_permissions_are_read_and_applied() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch_dir(usize::MAX - 2).expect("scratch directory");
        let outside = scratch_dir(usize::MAX - 3).expect("scratch directory");
        let file = dir.join("mode.txt");
        std::fs::write(&file, b"x").unwrap();
        let entries = read_local(&dir.display().to_string()).unwrap();
        let entry = entries.iter().find(|e| e.name == "mode.txt").unwrap();
        assert!(entry.perms.is_some(), "a POSIX host has to report a mode");

        let progress = Arc::new(Mutex::new(ztransfer::Progress::default()));
        let wake: remote::Wake = Arc::new(|| {});
        apply_local_mode(&file, 0o600, false, &progress, &wake).unwrap();
        assert_eq!(mode_of(&file), 0o600, "a single-file chmod has to land");

        // A recursive change reaches the tree but must not follow a symlink out
        // of it: the link's target lives elsewhere and stays untouched.
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let nested = sub.join("nested.txt");
        std::fs::write(&nested, b"y").unwrap();
        let target = outside.join("target.txt");
        std::fs::write(&target, b"z").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::os::unix::fs::symlink(&target, sub.join("link.txt")).unwrap();

        apply_local_mode(&dir, 0o750, true, &progress, &wake).unwrap();
        assert_eq!(
            mode_of(&nested),
            0o750,
            "a recursive chmod has to reach nested files"
        );
        assert_eq!(
            mode_of(&target),
            0o644,
            "a recursive chmod followed a symlink"
        );

        // A mode that drops the execute bit has to still succeed: the walk
        // changes the contents before the directory that holds them.
        apply_local_mode(&dir, 0o600, true, &progress, &wake).unwrap();
        assert_eq!(mode_of(&dir), 0o600);
        // Put traversal back, then clear the tree.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        apply_local_mode(&dir, 0o700, true, &progress, &wake).unwrap();

        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(outside).unwrap();
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::symlink_metadata(path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    }

    /// The local listing now runs on a worker thread, so its own rules are worth
    /// pinning: directories first, then a case-insensitive sort by name.
    #[test]
    fn local_listing_sorts_directories_first() {
        let dir = scratch_dir(usize::MAX - 1).expect("scratch directory");
        std::fs::create_dir_all(dir.join("zdir")).unwrap();
        std::fs::write(dir.join("b.txt"), b"bb").unwrap();
        std::fs::write(dir.join("A.txt"), b"a").unwrap();

        let rows = read_local(&dir.display().to_string()).unwrap();
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(
            names,
            ["zdir", "A.txt", "b.txt"],
            "directories first, then names case-insensitively"
        );
        let b = rows.iter().find(|row| row.name == "b.txt").unwrap();
        assert_eq!(b.size, 2, "the size has to come through");
        assert!(b.mtime.is_some(), "the timestamp has to come through");
        // A missing directory is an error to show, not a panic.
        assert!(read_local(&dir.join("absent").display().to_string()).is_err());

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The address bar is laid out right to left so the field takes what is left.
    /// The icons come after the field in that order, so their room has to be held
    /// back — without it the field eats the lot and the icons land at negative x,
    /// painting over the pane to the left of this one.
    #[test]
    fn the_address_bar_stays_inside_its_pane() {
        const PANE_LEFT: f32 = 300.0;
        const PANE_RIGHT: f32 = 470.0;
        let palette = Palette::new(false);
        let ctx = egui::Context::default();
        let mut bar: Option<egui::Rect> = None;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 300.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let pane = egui::Rect::from_min_max(
                        egui::pos2(PANE_LEFT, 0.0),
                        egui::pos2(PANE_RIGHT, 200.0),
                    );
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(pane)
                            .layout(egui::Layout::top_down(egui::Align::LEFT)),
                    );
                    let mut path = String::from("C:/Users/firmm");
                    child.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            crate::icons::icon_label_button(
                                ui,
                                crate::icons::Icon::Refresh,
                                "刷新",
                                palette,
                            );
                            let icons = crate::icons::Size::Button.button().x * 2.0
                                + ui.spacing().item_spacing.x * 2.0;
                            let room = ui.available_width();
                            // Same rule as the panes: the shortcuts go before the
                            // field is allowed to overflow.
                            let shortcuts = room - icons >= MIN_FIELD_WIDTH;
                            editing::field_with(ui, &mut path, |edit| {
                                edit.desired_width(if shortcuts { room - icons } else { room })
                            });
                            if shortcuts {
                                crate::icons::icon_button(
                                    ui,
                                    crate::icons::Icon::Home,
                                    palette,
                                    crate::icons::Size::Button,
                                );
                                crate::icons::icon_button(
                                    ui,
                                    crate::icons::Icon::FolderUp,
                                    palette,
                                    crate::icons::Size::Button,
                                );
                            }
                            bar = Some(ui.min_rect());
                        });
                    });
                });
            },
        );
        let bar = bar.expect("the bar was laid out");
        assert!(
            bar.left() >= PANE_LEFT - 0.5,
            "the bar started left of its pane: {bar:?}"
        );
        assert!(
            bar.right() <= PANE_RIGHT + 0.5,
            "the bar ran past its pane: {bar:?}"
        );
    }

    /// When the pane is narrower than the table needs, the scroll area has to
    /// report content wider than its viewport — that is the condition the
    /// horizontal bar appears under. If the content fits, no bar is drawn and the
    /// columns have nowhere to go but a truncation.
    #[test]
    fn a_narrow_pane_makes_the_table_overflow_sideways() {
        const VIEWPORT: f32 = 300.0;
        let ctx = egui::Context::default();
        let mut measured: Option<(f32, f32)> = None;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(VIEWPORT, 300.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let area =
                        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(VIEWPORT, 200.0));
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(area)
                            .layout(egui::Layout::top_down(egui::Align::LEFT)),
                    );
                    let cells = [
                        Cell {
                            text: "readme.txt".into(),
                            right: false,
                            color: egui::Color32::WHITE,
                            full: None,
                            icon: None,
                            link: false,
                        },
                        Cell {
                            text: "1.0 KiB".into(),
                            right: true,
                            color: egui::Color32::GRAY,
                            full: None,
                            icon: None,
                            link: false,
                        },
                        Cell {
                            text: "-rw-r--r--".into(),
                            right: true,
                            color: egui::Color32::GRAY,
                            full: None,
                            icon: None,
                            link: false,
                        },
                        Cell {
                            text: "03-14 20:32".into(),
                            right: true,
                            color: egui::Color32::GRAY,
                            full: None,
                            icon: None,
                            link: false,
                        },
                    ];
                    let output = egui::ScrollArea::both()
                        .id_salt("narrow-probe")
                        .auto_shrink([false, false])
                        .max_height(150.0)
                        .show(&mut child, |ui| {
                            let width = ui.available_width().max(MIN_TABLE_WIDTH);
                            table_row(ui, &cells, &REMOTE_COLUMNS, false, width);
                        });
                    measured = Some((output.content_size.x, output.inner_rect.width()));
                });
            },
        );
        let (content, viewport) = measured.expect("the table was laid out");
        assert!(
            content > viewport,
            "content is {content} wide in a {viewport} viewport, so no bar would appear"
        );
    }
}
