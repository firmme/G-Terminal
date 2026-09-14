use crate::config::RemoteProfile;
use anyhow::{Context, Result, bail, ensure};
use russh::{
    client,
    keys::{self, PrivateKeyWithHashAlg, ssh_key},
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::{Semaphore, oneshot},
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
    pub sftp: Option<Arc<russh_sftp::client::SftpSession>>,
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
    if !profile.identity.trim().is_empty() {
        let key = keys::load_secret_key(
            &profile.identity,
            if passphrase.is_empty() {
                None
            } else {
                Some(passphrase)
            },
        )
        .context("无法读取私钥")?;
        let hash = handle.best_supported_rsa_hash().await?.flatten();
        if handle
            .authenticate_publickey(&user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
            .await?
            .success()
        {
            return Ok(());
        }
    }
    if !password.is_empty()
        && handle
            .authenticate_password(&user, password)
            .await?
            .success()
    {
        return Ok(());
    }
    bail!("SSH 认证失败：检查用户名、密码或私钥；当前内置连接不自动读取 OpenSSH config / agent")
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
    // A missing or slow SFTP subsystem must not kill an authenticated session.
    let sftp = match tokio::time::timeout(Duration::from_secs(10), async {
        let channel = match handle.channel_open_session().await {
            Ok(channel) => channel,
            Err(_) => return None,
        };
        if channel.request_subsystem(true, "sftp").await.is_err() {
            return None;
        }
        russh_sftp::client::SftpSession::new(channel.into_stream())
            .await
            .ok()
            .map(Arc::new)
    })
    .await
    {
        Ok(Some(sftp)) => Some(sftp),
        _ => None,
    };
    let connection = Arc::new(Connection {
        handle: Arc::new(handle),
        profile,
        sftp,
        forwarding: Arc::new(Mutex::new(vec![])),
        transfer_gate: Arc::new(Semaphore::new(1)),
        jump,
        forward_tasks: Mutex::new(vec![]),
    });
    connection.start_forwards(wake).await;
    Ok(connection)
}

impl Connection {
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
    pub size: u64,
    /// POSIX permission bits, e.g. 0o644; None when unavailable.
    pub perms: Option<u32>,
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
            let sftp = connection.sftp.as_ref().ok_or_else(|| {
                anyhow::anyhow!("服务器未提供 SFTP 子系统，无法浏览文件；ZMODEM 传输仍可用")
            })?;
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
                        size: metadata.size.unwrap_or(0),
                        perms: metadata.permissions,
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
    pub connection: Arc<Connection>,
}

impl Transfer {
    pub fn new(
        connection: Arc<Connection>,
        local: PathBuf,
        remote: String,
        direction: Direction,
        wake: Wake,
    ) -> Self {
        let transfer = Self {
            local,
            remote,
            direction,
            connection,
            state: Arc::new(Mutex::new(TransferState {
                done: 0,
                total: 0,
                message: "排队中".into(),
                finished: false,
                running: false,
            })),
            pause: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        transfer.start(wake);
        transfer
    }
    pub fn start(&self, wake: Wake) {
        if self.state.lock().unwrap().running {
            return;
        }
        self.pause
            .store(false, std::sync::atomic::Ordering::Release);
        self.state.lock().unwrap().running = true;
        let (connection, local, remote, direction, state, pause) = (
            self.connection.clone(),
            self.local.clone(),
            self.remote.clone(),
            self.direction,
            self.state.clone(),
            self.pause.clone(),
        );
        runtime().spawn(async move {
            let result = async {
                let _permit = connection.transfer_gate.clone().acquire_owned().await?;
                transfer_file(
                    &connection,
                    &local,
                    &remote,
                    direction,
                    &state,
                    &pause,
                    &wake,
                )
                .await
            }
            .await;
            let mut s = state.lock().unwrap();
            s.running = false;
            match result {
                Ok(()) => {
                    s.finished = true;
                    s.message = "完成".into();
                }
                Err(e) => {
                    s.message = format!("{e:#}");
                }
            }
            wake();
        });
    }
}

async fn transfer_file(
    connection: &Connection,
    local: &PathBuf,
    remote: &str,
    direction: Direction,
    state: &Arc<Mutex<TransferState>>,
    pause: &std::sync::atomic::AtomicBool,
    wake: &Wake,
) -> Result<()> {
    use russh_sftp::protocol::OpenFlags;
    use std::io::SeekFrom;
    let sftp = connection.sftp.as_ref().ok_or_else(|| {
        anyhow::anyhow!("服务器未提供 SFTP 子系统，无法传输文件；ZMODEM 传输仍可用")
    })?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut verify = vec![0u8; 64 * 1024];
    let partial_remote = format!("{remote}.gterminal.part");
    let partial_local = PathBuf::from(format!("{}.gterminal.part", local.display()));
    ensure!(!pause.load(std::sync::atomic::Ordering::Acquire), "已暂停");
    let (mut source, mut destination, total, offset): (
        Box<dyn ReadSeek>,
        Box<dyn ReadWriteSeek>,
        u64,
        u64,
    ) = match direction {
        Direction::Upload => {
            ensure!(
                !sftp.try_exists(remote).await?,
                "目标已存在；请改名或删除目标后重试（不会自动覆盖）"
            );
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
            ensure!(
                !tokio::fs::try_exists(local).await?,
                "本地目标已存在；请改名后重试（不会自动覆盖）"
            );
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
        ensure!(!pause.load(std::sync::atomic::Ordering::Acquire), "已暂停");
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
        ensure!(!pause.load(std::sync::atomic::Ordering::Acquire), "已暂停");
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
            ensure!(
                !sftp.try_exists(remote).await?,
                "目标在传输期间已出现，保留临时文件"
            );
            sftp.rename(partial_remote, remote).await?;
        }
        Direction::Download => {
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
