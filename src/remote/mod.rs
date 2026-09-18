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

mod copy;
mod dir;
mod ops;
mod transfer;
use copy::*;
use dir::*;
pub use dir::{DirectoryState, Entry, directory_is_empty, list_directory, remove};
use ops::*;
pub use ops::{
    quote_posix, resolve_gid, resolve_uid, run_capture, safe_local_name, set_owner,
    set_permissions, walk_remote,
};
use transfer::*;
pub use transfer::{
    BatchPolicy, Conflict, ConflictChoice, Direction, PlannedFile, Transfer, TransferSpec,
    TransferState, create_dir, fetch, plan_download, plan_upload, put, rename,
};

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

#[cfg(test)]
mod tests;
