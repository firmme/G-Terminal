use crate::config::RemoteProfile;
use anyhow::{Context, Result, bail, ensure};
use russh::{
    client,
    keys::{self, PrivateKeyWithHashAlg, ssh_key},
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::{Semaphore, mpsc, oneshot},
    task::JoinHandle,
};
use zeroize::Zeroizing;

pub type Wake = Arc<dyn Fn() + Send + Sync>;
pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("Tokio runtime")
    })
}

#[derive(Default)]
pub struct Credentials {
    pub password: Zeroizing<String>,
    pub passphrase: Zeroizing<String>,
    pub jump_password: Zeroizing<String>,
}

pub struct TrustRequest {
    pub host: String,
    pub fingerprint: String,
    pub answer: Option<oneshot::Sender<bool>>,
}
#[derive(Default)]
pub struct ConnectState {
    pub message: String,
    pub trust: Option<TrustRequest>,
    pub ready: Option<Arc<Connection>>,
    pub error: Option<String>,
}
pub struct ConnectJob {
    pub state: Arc<Mutex<ConnectState>>,
    task: JoinHandle<()>,
}
impl Drop for ConnectJob {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub struct Client {
    host: String,
    port: u16,
    state: Arc<Mutex<ConnectState>>,
    wake: Wake,
    known_hosts: PathBuf,
}
impl client::Handler for Client {
    type Error = anyhow::Error;
    async fn check_server_key(&mut self, key: &ssh_key::PublicKey) -> Result<bool> {
        match keys::check_known_hosts_path(&self.host, self.port, key, &self.known_hosts) {
            Ok(true) => Ok(true),
            Err(e) => Err(anyhow::anyhow!(
                "{}:{} 主机密钥校验失败：{e}。请核实服务器变化后手动更新 known_hosts。",
                self.host,
                self.port
            )),
            Ok(false) => {
                let (send, receive) = oneshot::channel();
                self.state.lock().unwrap().trust = Some(TrustRequest {
                    host: format!("{}:{}", self.host, self.port),
                    fingerprint: key.fingerprint(ssh_key::HashAlg::Sha256).to_string(),
                    answer: Some(send),
                });
                (self.wake)();
                let accepted = tokio::time::timeout(Duration::from_secs(180), receive).await??;
                if accepted {
                    keys::known_hosts::learn_known_hosts_path(
                        &self.host,
                        self.port,
                        key,
                        &self.known_hosts,
                    )?;
                }
                Ok(accepted)
            }
        }
    }
}

pub struct Connection {
    pub handle: Arc<client::Handle<Client>>,
    pub profile: RemoteProfile,
    /// Opened on first use. A server without SFTP costs nothing at connect time,
    /// and caching the outcome keeps a failure from being re-probed on every call.
    sftp: tokio::sync::OnceCell<Option<Arc<russh_sftp::client::SftpSession>>>,
    pub forwarding: Arc<Mutex<Vec<String>>>,
    pub transfer_gate: Arc<Semaphore>,
    jump: Option<Arc<client::Handle<Client>>>,
    forward_tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        for task in self.forward_tasks.lock().unwrap().drain(..) {
            task.abort();
        }
    }
}

/// `keys::load_secret_key` opens the path verbatim, while the OpenSSH backend
/// hands it to `ssh.exe`, which expands `~`. Expand it here so both agree.
fn expand_identity(path: &str) -> PathBuf {
    let path = path.trim();
    let rest = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"));
    match (rest, directories::UserDirs::new()) {
        (Some(rest), Some(dirs)) => dirs.home_dir().join(rest),
        _ => PathBuf::from(path),
    }
}

/// The private keys an OpenSSH client tries when no identity is configured.
fn default_identities() -> Vec<PathBuf> {
    let Some(home) = directories::UserDirs::new().map(|d| d.home_dir().to_path_buf()) else {
        return Vec::new();
    };
    ["id_ed25519", "id_ecdsa", "id_rsa"]
        .into_iter()
        .map(|name| home.join(".ssh").join(name))
        .filter(|path| path.is_file())
        .collect()
}

fn load_identity(path: &Path, passphrase: &str) -> Result<keys::PrivateKey> {
    let attempt = |password: Option<&str>| keys::load_secret_key(path, password);
    let give_up = |e: keys::Error| match e {
        keys::Error::KeyIsEncrypted => {
            anyhow::anyhow!("私钥已加密，请在「私钥口令」中填写口令")
        }
        e => anyhow::Error::new(e).context("无法读取私钥"),
    };
    match attempt(if passphrase.is_empty() {
        None
    } else {
        Some(passphrase)
    }) {
        Ok(key) => Ok(key),
        // A passphrase supplied for an unencrypted key must not make it unusable.
        Err(_) if !passphrase.is_empty() => attempt(None).map_err(give_up),
        Err(e) => Err(give_up(e)),
    }
}

async fn authenticate(
    handle: &mut client::Handle<Client>,
    profile: &RemoteProfile,
    password: &str,
    passphrase: &str,
) -> Result<()> {
    let user = if profile.user.is_empty() {
        std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "root".into())
    } else {
        profile.user.clone()
    };
    if handle.authenticate_none(&user).await?.success() {
        return Ok(());
    }
    // The configured identity goes first, then the defaults, which is what an
    // OpenSSH client does without IdentitiesOnly.
    let mut candidates = Vec::new();
    if !profile.identity.trim().is_empty() {
        candidates.push(expand_identity(&profile.identity));
    }
    for path in default_identities() {
        if !candidates.contains(&path) {
            candidates.push(path);
        }
    }

    let mut failures = Vec::new();
    for path in candidates {
        let key = match load_identity(&path, passphrase) {
            Ok(key) => key,
            Err(e) => {
                failures.push(format!("{}：{e:#}", path.display()));
                continue;
            }
        };
        let hash = handle.best_supported_rsa_hash().await?.flatten();
        match handle
            .authenticate_publickey(&user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
            .await
        {
            Ok(result) if result.success() => return Ok(()),
            Ok(_) => failures.push(format!("{}：公钥被服务器拒绝", path.display())),
            Err(e) => failures.push(format!("{}：{e:#}", path.display())),
        }
    }

    if password.is_empty() {
        failures.push("未填写密码".into());
    } else if handle
        .authenticate_password(&user, password)
        .await?
        .success()
    {
        return Ok(());
    } else {
        failures.push("密码认证被服务器拒绝".into());
    }
    bail!("SSH 认证失败：\n  {}", failures.join("\n  "))
}

pub fn connect(profile: RemoteProfile, credentials: Credentials, wake: Wake) -> ConnectJob {
    let state = Arc::new(Mutex::new(ConnectState {
        message: "正在连接…".into(),
        ..Default::default()
    }));
    let target = state.clone();
    let task = runtime().spawn(async move {
        let result = connect_inner(profile, credentials, target.clone(), wake.clone(), None).await;
        let mut state = target.lock().unwrap();
        state.trust = None;
        match result {
            Ok(connection) => state.ready = Some(connection),
            Err(e) => state.error = Some(format!("{e:#}")),
        }
        wake();
    });
    ConnectJob { state, task }
}

async fn connect_inner(
    profile: RemoteProfile,
    credentials: Credentials,
    state: Arc<Mutex<ConnectState>>,
    wake: Wake,
    known_hosts: Option<PathBuf>,
) -> Result<Arc<Connection>> {
    profile.validate()?;
    let config = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(30)),
        keepalive_max: 3,
        ..Default::default()
    });
    let handler = |p: &RemoteProfile| Client {
        host: p.host.clone(),
        port: p.port,
        state: state.clone(),
        wake: wake.clone(),
        known_hosts: known_hosts.clone().unwrap_or_else(|| {
            directories::UserDirs::new()
                .map(|d| d.home_dir().join(".ssh/known_hosts"))
                .unwrap_or_else(|| PathBuf::from("known_hosts"))
        }),
    };
    let mut jump = None;
    let mut handle = if let Some(p) = &profile.jump {
        ensure!(p.jump.is_none(), "当前支持一级 ProxyJump");
        p.validate()?;
        let stream = tokio::time::timeout(
            Duration::from_secs(15),
            tokio::net::TcpStream::connect((p.host.as_str(), p.port)),
        )
        .await??;
        let mut h = client::connect_stream(config.clone(), stream, handler(p)).await?;
        authenticate(
            &mut h,
            p,
            &credentials.jump_password,
            &credentials.passphrase,
        )
        .await?;
        let channel = h
            .channel_open_direct_tcpip(&profile.host, profile.port as u32, "127.0.0.1", 0)
            .await?;
        jump = Some(Arc::new(h));
        client::connect_stream(config, channel.into_stream(), handler(&profile)).await?
    } else {
        let stream = tokio::time::timeout(
            Duration::from_secs(15),
            tokio::net::TcpStream::connect((profile.host.as_str(), profile.port)),
        )
        .await??;
        client::connect_stream(config, stream, handler(&profile)).await?
    };
    authenticate(
        &mut handle,
        &profile,
        &credentials.password,
        &credentials.passphrase,
    )
    .await?;
    let connection = Arc::new(Connection {
        handle: Arc::new(handle),
        profile,
        sftp: tokio::sync::OnceCell::new(),
        forwarding: Arc::new(Mutex::new(vec![])),
        transfer_gate: Arc::new(Semaphore::new(1)),
        jump,
        forward_tasks: Mutex::new(vec![]),
    });
    connection.start_forwards(wake).await;
    Ok(connection)
}

impl Connection {
    /// Opens the SFTP subsystem. A missing or slow subsystem must not kill an
    /// authenticated session, so every failure collapses to `None`.
    async fn open_sftp(&self) -> Option<Arc<russh_sftp::client::SftpSession>> {
        tokio::time::timeout(Duration::from_secs(10), async {
            let channel = self.handle.channel_open_session().await.ok()?;
            channel.request_subsystem(true, "sftp").await.ok()?;
            russh_sftp::client::SftpSession::new(channel.into_stream())
                .await
                .ok()
                .map(Arc::new)
        })
        .await
        .ok()
        .flatten()
    }

    /// The SFTP session, opened on first use. `action` names the operation for
    /// the error shown when the server has no SFTP subsystem.
    pub async fn sftp(&self, action: &str) -> Result<Arc<russh_sftp::client::SftpSession>> {
        match self.sftp.get_or_init(|| self.open_sftp()).await {
            Some(sftp) => Ok(sftp.clone()),
            None => bail!("服务器未提供 SFTP 子系统，无法{action}；ZMODEM 传输仍可用"),
        }
    }

    async fn start_forwards(self: &Arc<Self>, wake: Wake) {
        for forward in &self.profile.forwards {
            let label = format!(
                "127.0.0.1:{} → {}:{}",
                forward.bind_port, forward.target_host, forward.target_port
            );
            let listener =
                match tokio::net::TcpListener::bind(("127.0.0.1", forward.bind_port)).await {
                    Ok(listener) => listener,
                    Err(e) => {
                        self.forwarding
                            .lock()
                            .unwrap()
                            .push(format!("{label} 失败：{e}"));
                        continue;
                    }
                };
            self.forwarding.lock().unwrap().push(label);
            let handle = self.handle.clone();
            let jump = self.jump.clone();
            let f = forward.clone();
            let task = runtime().spawn(async move {
                let _jump = jump;
                let mut children = tokio::task::JoinSet::new();
                loop {
                    tokio::select! {
                        accepted = listener.accept() => {
                            let Ok((mut stream, peer)) = accepted else { break; };
                            let handle = handle.clone();
                            let f = f.clone();
                            children.spawn(async move {
                                if let Ok(channel) = handle.channel_open_direct_tcpip(&f.target_host, f.target_port as u32, peer.ip().to_string(), peer.port() as u32).await {
                                    let _ = tokio::io::copy_bidirectional(&mut stream, &mut channel.into_stream()).await;
                                }
                            });
                        }
                        _ = children.join_next(), if !children.is_empty() => {}
                    }
                }
            });
            self.forward_tasks.lock().unwrap().push(task);
        }
        wake();
    }

    pub fn stop_forwards(&self) {
        for task in self.forward_tasks.lock().unwrap().drain(..) {
            task.abort();
        }
        self.forwarding.lock().unwrap().clear();
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub directory: bool,
    /// A symlink. `directory` is false for a link to a directory, because the
    /// listing reports the link itself rather than what it points at.
    pub symlink: bool,
    pub size: u64,
    /// POSIX permission bits, e.g. 0o644; None when unavailable.
    pub perms: Option<u32>,
    /// Numeric owner and group, which is all SFTP carries. None when the server
    /// omits them.
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    /// Modification time in Unix seconds; None when the server omits it.
    pub mtime: Option<u32>,
}
#[derive(Default)]
pub struct DirectoryState {
    pub path: String,
    pub entries: Vec<Entry>,
    pub error: Option<String>,
    pub loading: bool,
}
pub fn list_directory(
    connection: Arc<Connection>,
    path: String,
    wake: Wake,
) -> Arc<Mutex<DirectoryState>> {
    let state = Arc::new(Mutex::new(DirectoryState {
        path: path.clone(),
        loading: true,
        ..Default::default()
    }));
    let output = state.clone();
    runtime().spawn(async move {
        let result: Result<_> = async {
            let sftp = connection.sftp("浏览文件").await?;
            let canonical = sftp.canonicalize(path).await?;
            let mut entries: Vec<_> = sftp
                .read_dir(&canonical)
                .await?
                .filter(|e| e.file_name() != "." && e.file_name() != "..")
                .map(|e| {
                    let metadata = e.metadata();
                    Entry {
                        name: e.file_name(),
                        directory: metadata.is_dir(),
                        // The listing reports the link itself, not its target.
                        symlink: e.file_type().is_symlink(),
                        size: metadata.size.unwrap_or(0),
                        perms: metadata.permissions,
                        uid: metadata.uid,
                        gid: metadata.gid,
                        mtime: metadata.mtime,
                    }
                })
                .collect();
            entries.sort_by(|a, b| {
                b.directory
                    .cmp(&a.directory)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            Ok((canonical, entries))
        }
        .await;
        let mut s = output.lock().unwrap();
        s.loading = false;
        match result {
            Ok((path, entries)) => {
                s.path = path;
                s.entries = entries;
            }
            Err(e) => s.error = Some(format!("{e:#}")),
        }
        wake();
    });
    state
}

/// Joins a child name onto a remote POSIX directory path.
fn join_remote(parent: &str, name: &str) -> String {
    format!("{}/{}", parent.trim_end_matches('/'), name)
}

/// Removes a remote file, or a directory and everything beneath it.
pub async fn remove(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    path: &str,
    directory: bool,
) -> Result<()> {
    if !directory {
        sftp.remove_file(path).await?;
        return Ok(());
    }
    // Collect breadth-first, then delete deepest-first: SFTP's `remove_dir` only
    // accepts an empty directory. A symlink is unlinked as a link and never
    // descended into, so a link pointing back at an ancestor cannot loop.
    let mut directories = vec![path.to_string()];
    let mut index = 0;
    while index < directories.len() {
        let dir = directories[index].clone();
        index += 1;
        for entry in sftp.read_dir(&dir).await? {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let child = join_remote(&dir, &name);
            if entry.file_type().is_dir() {
                directories.push(child);
            } else {
                sftp.remove_file(&child).await?;
            }
        }
    }
    for dir in directories.iter().rev() {
        sftp.remove_dir(dir).await?;
    }
    Ok(())
}

/// Whether a remote directory is empty, so a recursive delete can be confirmed
/// before it happens rather than after.
pub async fn directory_is_empty(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    path: &str,
) -> Result<bool> {
    for entry in sftp.read_dir(path).await? {
        let name = entry.file_name();
        if name != "." && name != ".." {
            return Ok(false);
        }
    }
    Ok(true)
}

/// One file a directory transfer will move. The size is known before anything
/// starts, which is what lets the queue show aggregate progress up front.
#[derive(Clone)]
pub struct PlannedFile {
    pub local: PathBuf,
    pub remote: String,
    pub size: u64,
}

/// A local tree walk: directories parent-first, then files with their sizes.
type LocalWalk = (Vec<PathBuf>, Vec<(PathBuf, u64)>);

/// Walks a local tree, returning its directories parent-first and then its files.
///
/// Symlinks are neither followed nor copied: a link is not the file it points at,
/// and following one walks out of the tree or loops.
fn walk_local(root: &Path) -> Result<LocalWalk> {
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
fn relative_to(root: &Path, path: &Path, remote: bool) -> Result<String> {
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
async fn move_one(
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
    overwrite: bool,
    pub connection: Arc<Connection>,
    batch_size: usize,
    conflicts: mpsc::UnboundedSender<Conflict>,
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
    overwrite_all: bool,
    rename_all: bool,
    skip_all: bool,
    cancelled: bool,
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
struct TransferControl {
    pause: Arc<std::sync::atomic::AtomicBool>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    skipped: Arc<std::sync::atomic::AtomicBool>,
    /// Allow replacing an existing target. Normal transfers ask first; the edit
    /// round-trip needs a silent replace, since every save overwrites the same file.
    overwrite: bool,
    policy: Arc<Mutex<BatchPolicy>>,
    conflicts: mpsc::UnboundedSender<Conflict>,
    batch_size: usize,
    batch: u64,
}

impl TransferControl {
    fn checkpoint(&self) -> Result<()> {
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

/// Where a transfer will actually land, once any conflict is settled.
struct Resolved {
    local: PathBuf,
    remote: String,
    /// An existing target is being replaced, so a partial left by an earlier
    /// attempt has to be discarded rather than resumed against.
    replacing: bool,
}

/// Decides the destination, asking the user when something is already there.
///
/// Runs before anything is opened: a rename has to move the `.part` sibling with
/// it, so this cannot wait until the transfer is under way.
async fn resolve_destination(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    local: &Path,
    remote: &str,
    direction: Direction,
    control: &TransferControl,
    wake: &Wake,
) -> Result<Resolved> {
    use std::sync::atomic::Ordering;
    let mut local = local.to_path_buf();
    let mut remote = remote.to_string();
    // A rename can land on another existing name; give up rather than spin.
    for _ in 0..8 {
        let (overwrite_all, rename_all, skip_all, cancelled) = {
            let policy = control.policy.lock().unwrap();
            (
                policy.overwrite_all,
                policy.rename_all,
                policy.skip_all,
                policy.cancelled,
            )
        };
        ensure!(!cancelled, "已取消");
        let Some(existing) = existing_size(sftp, &local, &remote, direction).await? else {
            return Ok(Resolved {
                local,
                remote,
                replacing: false,
            });
        };
        // From here on the target exists.
        if control.overwrite || overwrite_all {
            return Ok(Resolved {
                local,
                remote,
                replacing: true,
            });
        }
        if skip_all {
            control.skipped.store(true, Ordering::Release);
            bail!("已跳过");
        }
        if rename_all {
            rename_to_free_name(sftp, direction, &mut local, &mut remote).await?;
            continue;
        }
        let (send, receive) = oneshot::channel();
        let incoming = incoming_size(sftp, &local, &remote, direction).await?;
        if control
            .conflicts
            .send(Conflict {
                name: remote_file_name(&remote),
                existing,
                incoming,
                batch_size: control.batch_size,
                batch: control.batch,
                answer: Some(send),
            })
            .is_err()
        {
            bail!("文件窗口已关闭，无法确认是否覆盖");
        }
        (*wake)();
        let choice = receive.await.context("文件窗口已关闭")?;
        match choice {
            ConflictChoice::Overwrite => {
                return Ok(Resolved {
                    local,
                    remote,
                    replacing: true,
                });
            }
            ConflictChoice::OverwriteAll => {
                control.policy.lock().unwrap().overwrite_all = true;
                return Ok(Resolved {
                    local,
                    remote,
                    replacing: true,
                });
            }
            ConflictChoice::Rename => {
                rename_to_free_name(sftp, direction, &mut local, &mut remote).await?;
            }
            ConflictChoice::RenameAll => {
                control.policy.lock().unwrap().rename_all = true;
                rename_to_free_name(sftp, direction, &mut local, &mut remote).await?;
            }
            ConflictChoice::Skip => {
                control.skipped.store(true, Ordering::Release);
                bail!("已跳过");
            }
            ConflictChoice::SkipAll => {
                control.policy.lock().unwrap().skip_all = true;
                control.skipped.store(true, Ordering::Release);
                bail!("已跳过");
            }
            ConflictChoice::CancelRemaining => {
                control.policy.lock().unwrap().cancel_remaining();
                control.cancel.store(true, Ordering::Release);
                bail!("已取消");
            }
        }
    }
    bail!("同名文件过多，请手动换个名字")
}

/// Size of whatever is already at the destination, or None when nothing is.
async fn existing_size(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    local: &Path,
    remote: &str,
    direction: Direction,
) -> Result<Option<u64>> {
    match direction {
        Direction::Upload => {
            if !sftp.try_exists(remote).await? {
                return Ok(None);
            }
            Ok(sftp.metadata(remote).await?.size)
        }
        Direction::Download => Ok(tokio::fs::metadata(local).await.ok().map(|m| m.len())),
    }
}

/// Size of the incoming file, for the conflict dialog.
async fn incoming_size(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    local: &Path,
    remote: &str,
    direction: Direction,
) -> Result<u64> {
    Ok(match direction {
        Direction::Upload => tokio::fs::metadata(local).await?.len(),
        Direction::Download => sftp.metadata(remote).await?.size.unwrap_or(0),
    })
}

/// Renames the receiving side to the first free numbered name. Only that side
/// moves: the sending side is the source and keeps its name, or the transfer goes
/// looking for a file nobody wrote.
async fn rename_to_free_name(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    direction: Direction,
    local: &mut PathBuf,
    remote: &mut String,
) -> Result<()> {
    let candidate = free_name(sftp, direction, local, remote).await?;
    match direction {
        Direction::Upload => *remote = join_remote(&parent_remote(remote.as_str()), &candidate),
        Direction::Download => {
            *local = local.parent().unwrap_or(Path::new("")).join(&candidate);
        }
    }
    Ok(())
}

/// The first `name (n).ext` that is still free on the receiving side.
async fn free_name(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    direction: Direction,
    local: &Path,
    remote: &str,
) -> Result<String> {
    let name = remote_file_name(remote);
    for index in 1..10_000u32 {
        let candidate = numbered(&name, index);
        let taken = match direction {
            Direction::Upload => {
                sftp.try_exists(join_remote(&parent_remote(remote), &candidate))
                    .await?
            }
            // An unreadable local path counts as taken, which errs toward asking
            // for the next name rather than clobbering something.
            Direction::Download => {
                tokio::fs::try_exists(local.parent().unwrap_or(Path::new("")).join(&candidate))
                    .await
                    .unwrap_or(true)
            }
        };
        if !taken {
            return Ok(candidate);
        }
    }
    bail!("同名文件过多，无法自动重命名")
}

/// `report.txt` at 2 becomes `report (2).txt`. A leading dot starts the name, not
/// an extension, so `.bashrc` stays whole.
fn numbered(name: &str, index: u32) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => {
            format!("{stem} ({index}).{ext}")
        }
        _ => format!("{name} ({index})"),
    }
}

fn remote_file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// The directory holding an absolute POSIX path.
fn parent_remote(path: &str) -> String {
    match path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(index) => path[..index].to_string(),
    }
}

async fn transfer_file(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    local: &Path,
    remote: &str,
    direction: Direction,
    state: &Arc<Mutex<TransferState>>,
    control: &TransferControl,
    wake: &Wake,
) -> Result<()> {
    use russh_sftp::protocol::OpenFlags;
    use std::io::SeekFrom;
    // Decide the target before anything is opened. A rename changes not just the
    // destination but the `.part` sibling derived from it, so resolving later
    // would leave the partial file under the name the user just declined.
    let decided = resolve_destination(sftp, local, remote, direction, control, wake).await?;
    let local = &decided.local;
    let remote = decided.remote.as_str();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut verify = vec![0u8; 64 * 1024];
    let partial_remote = format!("{remote}.gterminal.part");
    let partial_local = PathBuf::from(format!("{}.gterminal.part", local.display()));
    control.checkpoint()?;
    let (mut source, mut destination, total, offset): (
        Box<dyn ReadSeek>,
        Box<dyn ReadWriteSeek>,
        u64,
        u64,
    ) = match direction {
        Direction::Upload => {
            if decided.replacing {
                // Replacing means different bytes, so a partial left by an earlier
                // pass must not be resumed against — its prefix would not match and
                // the transfer would refuse to continue.
                let _ = sftp.remove_file(&partial_remote).await;
            }
            let source = tokio::fs::File::open(local).await?;
            let total = source.metadata().await?.len();
            let file = sftp
                .open_with_flags(
                    &partial_remote,
                    OpenFlags::CREATE | OpenFlags::READ | OpenFlags::WRITE,
                )
                .await?;
            let offset = file.metadata().await?.size.unwrap_or(0);
            (Box::new(source), Box::new(file), total, offset)
        }
        Direction::Download => {
            if decided.replacing {
                let _ = tokio::fs::remove_file(&partial_local).await;
            }
            let source = sftp.open(remote).await?;
            let total = source.metadata().await?.size.unwrap_or(0);
            let file = tokio::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&partial_local)
                .await?;
            let offset = file.metadata().await?.len();
            (Box::new(source), Box::new(file), total, offset)
        }
    };
    ensure!(
        offset <= total,
        "断点文件大于源文件，请清理 .gterminal.part 后重试"
    );
    {
        let mut s = state.lock().unwrap();
        s.total = total;
        s.done = 0;
        s.message = "校验断点前缀".into();
    }
    let mut checked = 0;
    while checked < offset {
        control.checkpoint()?;
        let n = (offset - checked).min(buffer.len() as u64) as usize;
        source.read_exact(&mut buffer[..n]).await?;
        destination.read_exact(&mut verify[..n]).await?;
        ensure!(
            buffer[..n] == verify[..n],
            "断点内容与源文件不一致，拒绝续传；请清理 .gterminal.part"
        );
        checked += n as u64;
    }
    source.seek(SeekFrom::Start(offset)).await?;
    destination.seek(SeekFrom::Start(offset)).await?;
    let mut done = offset;
    while done < total {
        control.checkpoint()?;
        let max = (total - done).min(buffer.len() as u64) as usize;
        let n = source.read(&mut buffer[..max]).await?;
        ensure!(n > 0, "源文件提前结束");
        destination.write_all(&buffer[..n]).await?;
        done += n as u64;
        {
            let mut s = state.lock().unwrap();
            s.done = done;
            s.message = "传输中".into();
        }
        wake();
    }
    destination.flush().await?;
    destination.shutdown().await?;
    drop(destination);
    drop(source);
    match direction {
        Direction::Upload => {
            if decided.replacing {
                // Some servers refuse to rename onto an existing path, which is
                // exactly the case here, so clear the target explicitly.
                let _ = sftp.remove_file(remote).await;
            } else {
                ensure!(
                    !sftp.try_exists(remote).await?,
                    "目标在传输期间已出现，保留临时文件"
                );
            }
            sftp.rename(partial_remote, remote).await?;
        }
        Direction::Download => {
            if decided.replacing {
                // `hard_link` refuses an existing target, so clear the one the user
                // agreed to replace before committing.
                let _ = tokio::fs::remove_file(local).await;
            }
            tokio::fs::hard_link(&partial_local, local)
                .await
                .context("提交下载失败，保留断点文件（目标必须不存在且文件系统支持硬链接）")?;
            tokio::fs::remove_file(partial_local).await?;
        }
    }
    Ok(())
}

trait ReadSeek: tokio::io::AsyncRead + tokio::io::AsyncSeek + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncSeek + Unpin + Send> ReadSeek for T {}
trait ReadWriteSeek:
    tokio::io::AsyncRead + tokio::io::AsyncWrite + tokio::io::AsyncSeek + Unpin + Send
{
}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + tokio::io::AsyncSeek + Unpin + Send>
    ReadWriteSeek for T
{
}

pub fn quote_posix(value: &str) -> Result<String> {
    ensure!(!value.contains('\0'), "路径不能包含 NUL");
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}

/// Runs a command on a fresh channel and returns its stdout.
///
/// `command` is passed to the remote shell verbatim, so every caller has to quote
/// whatever it interpolates — see [`quote_posix`].
pub async fn run_capture(connection: &Arc<Connection>, command: &str) -> Result<String> {
    let mut channel = connection.handle.channel_open_session().await?;
    channel.exec(true, command).await?;
    let mut output = String::new();
    let mut code = None;
    while let Some(message) = channel.wait().await {
        match message {
            russh::ChannelMsg::Data { data } | russh::ChannelMsg::ExtendedData { data, .. } => {
                // Bounded: this is for reading a small value back, not a stream.
                if output.len() < 4096 {
                    output.push_str(&String::from_utf8_lossy(&data));
                }
            }
            russh::ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status),
            _ => {}
        }
    }
    ensure!(code == Some(0), "命令失败：{}", output.trim());
    Ok(output)
}

/// Resolves a user name to its numeric id. A number is taken as-is, because SFTP
/// carries ids and not names.
pub async fn resolve_uid(connection: &Arc<Connection>, name: &str) -> Result<u32> {
    let name = name.trim();
    if let Ok(id) = name.parse() {
        return Ok(id);
    }
    ensure!(!name.is_empty(), "请填写所有者");
    let output = run_capture(connection, &format!("id -u {}", quote_posix(name)?)).await?;
    output
        .trim()
        .parse()
        .with_context(|| format!("无法解析用户 {name} 的 uid"))
}

/// Resolves a group name to its numeric id. A number is taken as-is.
///
/// `getent group` is the direct lookup; `id -g` is the fallback for a system
/// without `getent`, where the name is more likely a user whose primary group is
/// wanted.
pub async fn resolve_gid(connection: &Arc<Connection>, name: &str) -> Result<u32> {
    let name = name.trim();
    if let Ok(id) = name.parse() {
        return Ok(id);
    }
    ensure!(!name.is_empty(), "请填写用户组");
    let quoted = quote_posix(name)?;
    let command = format!("getent group {quoted} 2>/dev/null | cut -d: -f3 || id -g {quoted}");
    let output = run_capture(connection, &command).await?;
    output
        .trim()
        .parse()
        .with_context(|| format!("无法解析用户组 {name} 的 gid"))
}

/// An attribute set that asks for nothing.
///
/// `FileAttributes::default()` is **not** empty: it carries `uid: Some(0)`,
/// `gid: Some(0)`, a mode and timestamps. Spreading it into a request would make
/// a plain chmod also try to take ownership of the file, so every field that
/// should be left alone has to be named as `None` here.
fn no_attributes() -> russh_sftp::protocol::FileAttributes {
    russh_sftp::protocol::FileAttributes {
        size: None,
        uid: None,
        user: None,
        gid: None,
        group: None,
        permissions: None,
        atime: None,
        mtime: None,
    }
}

/// Changes a remote entry's mode, and nothing else.
pub async fn set_permissions(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    path: &str,
    mode: u32,
) -> Result<()> {
    let mut attrs = no_attributes();
    attrs.permissions = Some(mode);
    sftp.set_metadata(path, attrs).await?;
    Ok(())
}

/// Changes a remote entry's owner and group.
///
/// Separate from [`set_permissions`] on purpose: SFTP carries both in one SETSTAT,
/// but a server refuses the ownership half for anyone who is not root. Sent
/// together, that refusal would take the mode change down with it.
pub async fn set_owner(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    path: &str,
    uid: Option<u32>,
    gid: Option<u32>,
) -> Result<()> {
    let mut attrs = no_attributes();
    attrs.uid = uid;
    attrs.gid = gid;
    sftp.set_metadata(path, attrs).await?;
    Ok(())
}

/// Every path at or below `root`, parents before their children.
///
/// A symlink is listed but never descended into. `read_dir` follows links, so
/// descending into one that points at an ancestor would walk forever — the same
/// reason the recursive delete refuses to follow them.
pub async fn walk_remote(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    root: &str,
) -> Result<Vec<String>> {
    let mut paths = vec![root.to_string()];
    let mut queue = std::collections::VecDeque::from([root.to_string()]);
    while let Some(directory) = queue.pop_front() {
        // A file has no children; asking would only raise an error.
        let Ok(entries) = sftp.read_dir(&directory).await else {
            continue;
        };
        for entry in entries {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let child = join_remote(&directory, &name);
            // Only what the listing calls a directory is descended into. The check
            // is on the link itself, not on what it points at.
            if entry.file_type().is_dir() {
                queue.push_back(child.clone());
            }
            paths.push(child);
        }
    }
    Ok(paths)
}

/// A remote basename must remain one ordinary local filename on Windows too.
pub fn safe_local_name(name: &str) -> bool {
    if name.is_empty()
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    {
        return false;
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;
