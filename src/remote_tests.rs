use super::*;
use russh::{
    Channel, ChannelId,
    server::{self, Msg},
};
use russh_sftp::protocol::*;
use std::collections::HashMap;
type Store = Arc<Mutex<HashMap<String, Vec<u8>>>>;
/// Paths that exist without holding bytes.
type Nodes = Arc<Mutex<HashMap<String, Node>>>;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Node {
    Dir,
    /// A symlink. The recursive delete must unlink it rather than follow it,
    /// otherwise a link pointing at an ancestor walks forever.
    Link,
}

/// Attributes a `setstat` has explicitly set, so `stat` can report them back. A
/// field left as `None` falls back to the listing default, which is what makes a
/// mode or ownership change observable.
#[derive(Clone, Copy, Default)]
struct SetAttrs {
    permissions: Option<u32>,
    uid: Option<u32>,
    gid: Option<u32>,
}

/// Explicitly set attributes, by path.
type SetMap = Arc<Mutex<HashMap<String, SetAttrs>>>;

struct TestSsh {
    channels: HashMap<ChannelId, Channel<Msg>>,
    store: Store,
    nodes: Nodes,
    attrs: SetMap,
    /// Whether `setstat` may change ownership. A real server refuses it for anyone
    /// who is not root, which is the case worth exercising.
    allow_chown: bool,
    shells: std::collections::HashSet<ChannelId>,
    /// Public keys this server accepts, so the publickey path is exercised at all.
    authorized: Vec<ssh_key::PublicKey>,
}

/// A control for tests that drive `transfer_file` directly. There is no UI to ask,
/// so conflicts are settled by `overwrite` up front — which also means the
/// conflict channel is a sink nobody reads.
fn test_control(overwrite: bool) -> TransferControl {
    TransferControl {
        pause: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        skipped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        overwrite,
        policy: Arc::new(Mutex::new(BatchPolicy::default())),
        conflicts: tokio::sync::mpsc::unbounded_channel().0,
        batch_size: 1,
        batch: 0,
    }
}
impl server::Handler for TestSsh {
    type Error = anyhow::Error;
    async fn auth_password(&mut self, user: &str, password: &str) -> Result<server::Auth> {
        Ok(if user == "test" && password == "test-password" {
            server::Auth::Accept
        } else {
            server::Auth::reject()
        })
    }
    async fn auth_publickey(
        &mut self,
        user: &str,
        public_key: &ssh_key::PublicKey,
    ) -> Result<server::Auth> {
        Ok(if user == "test" && self.authorized.contains(public_key) {
            server::Auth::Accept
        } else {
            server::Auth::reject()
        })
    }
    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        _: &mut server::Session,
    ) -> Result<bool> {
        self.channels.insert(channel.id(), channel);
        Ok(true)
    }
    async fn subsystem_request(
        &mut self,
        id: ChannelId,
        name: &str,
        session: &mut server::Session,
    ) -> Result<()> {
        ensure!(name == "sftp", "Expected SFTP");
        let channel = self.channels.remove(&id).unwrap();
        session.channel_success(id)?;
        let store = self.store.clone();
        let nodes = self.nodes.clone();
        let attrs = self.attrs.clone();
        let allow_chown = self.allow_chown;
        tokio::spawn(async move {
            russh_sftp::server::run(
                channel.into_stream(),
                TestSftp {
                    store,
                    nodes,
                    attrs,
                    allow_chown,
                    listing: None,
                    read: false,
                },
            )
            .await;
        });
        Ok(())
    }
    async fn pty_request(
        &mut self,
        id: ChannelId,
        _: &str,
        _: u32,
        _: u32,
        _: u32,
        _: u32,
        _: &[(russh::Pty, u32)],
        session: &mut server::Session,
    ) -> Result<()> {
        session.channel_success(id)?;
        Ok(())
    }
    async fn shell_request(&mut self, id: ChannelId, session: &mut server::Session) -> Result<()> {
        self.shells.insert(id);
        session.channel_success(id)?;
        session.data(id, b"test-ready\r\n".to_vec())?;
        Ok(())
    }
    async fn data(
        &mut self,
        id: ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> Result<()> {
        if self.shells.contains(&id) {
            session.data(id, data.to_vec())?;
        }
        Ok(())
    }
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host: &str,
        port: u32,
        _: &str,
        _: u32,
        _: &mut server::Session,
    ) -> Result<bool> {
        ensure!(host == "127.0.0.1", "Test may only connect to loopback");
        let mut stream = tokio::net::TcpStream::connect((host, port as u16)).await?;
        tokio::spawn(async move {
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut channel.into_stream()).await;
        });
        Ok(true)
    }
}
struct TestSftp {
    store: Store,
    nodes: Nodes,
    attrs: SetMap,
    allow_chown: bool,
    /// Path being listed, remembered from `opendir`.
    listing: Option<String>,
    read: bool,
}
fn ok(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: String::new(),
        language_tag: String::new(),
    }
}
/// The containing directory of an absolute POSIX path.
fn parent_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) | None => "/",
        Some(index) => &path[..index],
    }
}
fn base_name(path: &str) -> &str {
    match path.rfind('/') {
        Some(index) => &path[index + 1..],
        None => path,
    }
}
/// Direct children of `parent` as (base name, kind), which is what `read_dir`
/// hands back and what the recursive delete walks.
fn children_of(store: &Store, nodes: &Nodes, parent: &str) -> Vec<(String, Option<Node>)> {
    let parent = parent.trim_end_matches('/');
    let parent = if parent.is_empty() { "/" } else { parent };
    let mut out: Vec<(String, Option<Node>)> = store
        .lock()
        .unwrap()
        .keys()
        .filter(|path| parent_of(path) == parent)
        .map(|path| (base_name(path).to_string(), None))
        .collect();
    out.extend(
        nodes
            .lock()
            .unwrap()
            .iter()
            .filter(|(path, _)| parent_of(path) == parent)
            .map(|(path, node)| (base_name(path).to_string(), Some(*node))),
    );
    out
}
impl russh_sftp::server::Handler for TestSftp {
    type Error = StatusCode;
    fn unimplemented(&self) -> StatusCode {
        StatusCode::OpUnsupported
    }
    async fn realpath(&mut self, id: u32, _: String) -> std::result::Result<Name, StatusCode> {
        Ok(Name {
            id,
            files: vec![File::dummy("/")],
        })
    }
    async fn opendir(&mut self, id: u32, path: String) -> std::result::Result<Handle, StatusCode> {
        self.read = false;
        self.listing = Some(path.clone());
        Ok(Handle { id, handle: path })
    }
    async fn readdir(&mut self, id: u32, _: String) -> std::result::Result<Name, StatusCode> {
        if self.read {
            return Err(StatusCode::Eof);
        }
        self.read = true;
        let Some(listing) = self.listing.clone() else {
            return Err(StatusCode::Failure);
        };
        let children = children_of(&self.store, &self.nodes, &listing);
        let store = self.store.lock().unwrap();
        Ok(Name {
            id,
            files: children
                .into_iter()
                .map(|(name, node)| {
                    // A file reports its real length; directories and links do not.
                    let size = match node {
                        None => store
                            .get(&join_remote(&listing, &name))
                            .map_or(0, |data| data.len()),
                        Some(_) => 0,
                    };
                    File::new(
                        name,
                        FileAttributes {
                            size: Some(size as u64),
                            // The client derives each entry's type from these bits,
                            // so directories and symlinks have to say so here.
                            permissions: Some(match node {
                                None => 0o100644,
                                Some(Node::Dir) => 0o040755,
                                Some(Node::Link) => 0o120777,
                            }),
                            ..Default::default()
                        },
                    )
                })
                .collect(),
        })
    }
    async fn remove(
        &mut self,
        id: u32,
        filename: String,
    ) -> std::result::Result<Status, StatusCode> {
        if self.store.lock().unwrap().remove(&filename).is_some() {
            return Ok(ok(id));
        }
        // A symlink unlinks like a file. A directory does not, which is the
        // distinction the recursive delete relies on.
        let mut nodes = self.nodes.lock().unwrap();
        if nodes.get(&filename) == Some(&Node::Link) {
            nodes.remove(&filename);
            return Ok(ok(id));
        }
        Err(StatusCode::NoSuchFile)
    }
    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _: FileAttributes,
    ) -> std::result::Result<Status, StatusCode> {
        self.nodes.lock().unwrap().insert(path, Node::Dir);
        Ok(ok(id))
    }
    async fn rmdir(&mut self, id: u32, path: String) -> std::result::Result<Status, StatusCode> {
        if !children_of(&self.store, &self.nodes, &path).is_empty() {
            return Err(StatusCode::Failure);
        }
        let removed = self.nodes.lock().unwrap().remove(&path);
        if removed != Some(Node::Dir) {
            return Err(StatusCode::NoSuchFile);
        }
        Ok(ok(id))
    }
    async fn open(
        &mut self,
        id: u32,
        filename: String,
        flags: OpenFlags,
        _: FileAttributes,
    ) -> std::result::Result<Handle, StatusCode> {
        let mut store = self.store.lock().unwrap();
        if flags.contains(OpenFlags::CREATE) {
            store.entry(filename.clone()).or_default();
        }
        if !store.contains_key(&filename) {
            return Err(StatusCode::NoSuchFile);
        }
        Ok(Handle {
            id,
            handle: filename,
        })
    }
    async fn close(&mut self, id: u32, _: String) -> std::result::Result<Status, StatusCode> {
        Ok(ok(id))
    }
    async fn stat(&mut self, id: u32, path: String) -> std::result::Result<Attrs, StatusCode> {
        let set = self.attrs.lock().unwrap().get(&path).copied();
        let s = self.store.lock().unwrap();
        let attrs = match s.get(&path) {
            Some(data) => FileAttributes {
                size: Some(data.len() as u64),
                permissions: Some(set.and_then(|a| a.permissions).unwrap_or(0o100644)),
                uid: set.and_then(|a| a.uid),
                gid: set.and_then(|a| a.gid),
                ..Default::default()
            },
            None if let Some(node) = self.nodes.lock().unwrap().get(&path).copied() => {
                FileAttributes {
                    size: Some(0),
                    permissions: Some(set.and_then(|a| a.permissions).unwrap_or(match node {
                        Node::Dir => 0o040755,
                        Node::Link => 0o120777,
                    })),
                    uid: set.and_then(|a| a.uid),
                    gid: set.and_then(|a| a.gid),
                    ..Default::default()
                }
            }
            None => return Err(StatusCode::NoSuchFile),
        };
        Ok(Attrs { id, attrs })
    }
    async fn lstat(&mut self, id: u32, path: String) -> std::result::Result<Attrs, StatusCode> {
        self.stat(id, path).await
    }
    async fn fstat(&mut self, id: u32, path: String) -> std::result::Result<Attrs, StatusCode> {
        self.stat(id, path).await
    }
    async fn setstat(
        &mut self,
        id: u32,
        path: String,
        attrs: FileAttributes,
    ) -> std::result::Result<Status, StatusCode> {
        // A real server refuses the ownership half for anyone who is not root,
        // and refuses the whole request when it carries one.
        if !self.allow_chown && (attrs.uid.is_some() || attrs.gid.is_some()) {
            return Err(StatusCode::PermissionDenied);
        }
        let exists = self.store.lock().unwrap().contains_key(&path)
            || self.nodes.lock().unwrap().contains_key(&path);
        if !exists {
            return Err(StatusCode::NoSuchFile);
        }
        let mut all = self.attrs.lock().unwrap();
        let entry = all.entry(path).or_default();
        if attrs.permissions.is_some() {
            entry.permissions = attrs.permissions;
        }
        if attrs.uid.is_some() {
            entry.uid = attrs.uid;
        }
        if attrs.gid.is_some() {
            entry.gid = attrs.gid;
        }
        Ok(ok(id))
    }
    async fn read(
        &mut self,
        id: u32,
        path: String,
        offset: u64,
        len: u32,
    ) -> std::result::Result<Data, StatusCode> {
        let s = self.store.lock().unwrap();
        let d = s.get(&path).ok_or(StatusCode::NoSuchFile)?;
        if offset as usize >= d.len() {
            return Err(StatusCode::Eof);
        }
        Ok(Data {
            id,
            data: d[offset as usize..(offset as usize + len as usize).min(d.len())].to_vec(),
        })
    }
    async fn write(
        &mut self,
        id: u32,
        path: String,
        offset: u64,
        data: Vec<u8>,
    ) -> std::result::Result<Status, StatusCode> {
        let mut s = self.store.lock().unwrap();
        let d = s.get_mut(&path).ok_or(StatusCode::NoSuchFile)?;
        let end = offset as usize + data.len();
        d.resize(d.len().max(end), 0);
        d[offset as usize..end].copy_from_slice(&data);
        Ok(ok(id))
    }
    async fn rename(
        &mut self,
        id: u32,
        old: String,
        new: String,
    ) -> std::result::Result<Status, StatusCode> {
        let mut s = self.store.lock().unwrap();
        if s.contains_key(&new) {
            return Err(StatusCode::Failure);
        }
        let data = s.remove(&old).ok_or(StatusCode::NoSuchFile)?;
        s.insert(new, data);
        Ok(ok(id))
    }
}

async fn test_server(
    key: keys::PrivateKey,
    store: Store,
    nodes: Nodes,
    authorized: Vec<ssh_key::PublicKey>,
) -> (u16, JoinHandle<()>) {
    test_server_opts(key, store, nodes, authorized, false).await
}

/// As `test_server`, but says whether the server will let ownership be changed.
/// A real one refuses for anyone who is not root, which is the case worth testing.
async fn test_server_opts(
    key: keys::PrivateKey,
    store: Store,
    nodes: Nodes,
    authorized: Vec<ssh_key::PublicKey>,
    allow_chown: bool,
) -> (u16, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = Arc::new(server::Config {
        keys: vec![key],
        auth_rejection_time: Duration::ZERO,
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    });
    let attrs: SetMap = Arc::new(Mutex::new(HashMap::new()));
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let config = config.clone();
            let store = store.clone();
            let nodes = nodes.clone();
            let attrs = attrs.clone();
            let authorized = authorized.clone();
            tokio::spawn(async move {
                let running = server::run_stream(
                    config,
                    stream,
                    TestSsh {
                        channels: HashMap::new(),
                        store,
                        nodes,
                        attrs,
                        allow_chown,
                        shells: std::collections::HashSet::new(),
                        authorized,
                    },
                )
                .await
                .unwrap();
                let _ = running.await;
            });
        }
    });
    (port, task)
}

fn empty_nodes() -> Nodes {
    Arc::new(Mutex::new(HashMap::new()))
}

#[test]
fn native_ssh_sftp_resume_proxyjump_and_forwarding() {
    runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(30), async {
            let key = keys::PrivateKey::random(
                &mut ssh_key::rand_core::OsRng,
                ssh_key::Algorithm::Ed25519,
            )
            .unwrap();
            let store: Store = Arc::new(Mutex::new(HashMap::new()));
            let (port, server) =
                test_server(key.clone(), store.clone(), empty_nodes(), vec![]).await;
            let (jump_port, jump_server) =
                test_server(key.clone(), store.clone(), empty_nodes(), vec![]).await;
            let dir = std::env::temp_dir().join(format!("gterminal-ssh-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let known = dir.join("known_hosts");
            keys::known_hosts::learn_known_hosts_path("127.0.0.1", port, key.public_key(), &known)
                .unwrap();
            keys::known_hosts::learn_known_hosts_path(
                "127.0.0.1",
                jump_port,
                key.public_key(),
                &known,
            )
            .unwrap();
            let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let echo_port = echo.local_addr().unwrap().port();
            let echo_task = tokio::spawn(async move {
                let (mut socket, _) = echo.accept().await.unwrap();
                let (mut r, mut w) = socket.split();
                tokio::io::copy(&mut r, &mut w).await.unwrap();
            });
            let ephemeral = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let forward_port = ephemeral.local_addr().unwrap().port();
            drop(ephemeral);
            let jump = RemoteProfile {
                host: "127.0.0.1".into(),
                port: jump_port,
                user: "test".into(),
                ..Default::default()
            };
            let profile = RemoteProfile {
                host: "127.0.0.1".into(),
                port,
                user: "test".into(),
                jump: Some(Box::new(jump)),
                forwards: vec![crate::config::Forward {
                    bind_port: forward_port,
                    target_host: "127.0.0.1".into(),
                    target_port: echo_port,
                }],
                ..Default::default()
            };
            let credentials = Credentials {
                password: Zeroizing::new("test-password".into()),
                jump_password: Zeroizing::new("test-password".into()),
                ..Default::default()
            };
            let connection = connect_inner(
                profile,
                credentials,
                Arc::new(Mutex::new(ConnectState::default())),
                Arc::new(|| {}),
                Some(known.clone()),
            )
            .await
            .unwrap();
            let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", forward_port))
                .await
                .unwrap();
            socket.write_all(b"forward-ok").await.unwrap();
            let mut echo = [0; 10];
            socket.read_exact(&mut echo).await.unwrap();
            assert_eq!(&echo, b"forward-ok");
            drop(socket);
            let source = dir.join("source.bin");
            let destination = dir.join("download.bin");
            let data: Vec<_> = (0..180000).map(|i| (i % 251) as u8).collect();
            std::fs::write(&source, &data).unwrap();
            store
                .lock()
                .unwrap()
                .insert("/data.bin.gterminal.part".into(), data[..50000].to_vec());
            let state = Arc::new(Mutex::new(TransferState {
                done: 0,
                total: 0,
                message: String::new(),
                running: true,
                finished: false,
            }));
            let wake: Wake = Arc::new(|| {});
            let control = test_control(false);
            // SFTP is opened on first use, so the test asks for it the same way the UI does.
            let sftp = connection.sftp("传输文件").await.unwrap();
            transfer_file(
                &sftp,
                &source,
                "/data.bin",
                Direction::Upload,
                &state,
                &control,
                &wake,
            )
            .await
            .unwrap();
            assert_eq!(store.lock().unwrap().get("/data.bin").unwrap(), &data);
            let part = PathBuf::from(format!("{}.gterminal.part", destination.display()));
            std::fs::write(&part, &data[..32000]).unwrap();
            transfer_file(
                &sftp,
                &destination,
                "/data.bin",
                Direction::Download,
                &state,
                &control,
                &wake,
            )
            .await
            .unwrap();
            assert_eq!(std::fs::read(&destination).unwrap(), data);
            assert!(
                transfer_file(
                    &sftp,
                    &source,
                    "/data.bin",
                    Direction::Upload,
                    &state,
                    &control,
                    &wake
                )
                .await
                .is_err()
            );
            store
                .lock()
                .unwrap()
                .insert("/wrong.bin.gterminal.part".into(), b"wrong".to_vec());
            assert!(
                transfer_file(
                    &sftp,
                    &source,
                    "/wrong.bin",
                    Direction::Upload,
                    &state,
                    &control,
                    &wake
                )
                .await
                .is_err()
            );
            let session = crate::session::Session::from_remote(connection.clone(), 100, wake);
            session.write(b"SSH-INPUT".to_vec()).unwrap();
            let start = std::time::Instant::now();
            loop {
                if session
                    .terminal
                    .lock()
                    .unwrap()
                    .parser
                    .screen()
                    .contents()
                    .contains("SSH-INPUT")
                {
                    break;
                }
                assert!(start.elapsed() < Duration::from_secs(5));
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let changed = keys::PrivateKey::random(
                &mut ssh_key::rand_core::OsRng,
                ssh_key::Algorithm::Ed25519,
            )
            .unwrap();
            let trust_state = Arc::new(Mutex::new(ConnectState::default()));
            let mut verifier = Client {
                host: "127.0.0.1".into(),
                port,
                state: trust_state.clone(),
                wake: Arc::new(|| {}),
                known_hosts: known.clone(),
            };
            use russh::client::Handler as _;
            assert!(
                verifier
                    .check_server_key(changed.public_key())
                    .await
                    .is_err()
            );
            assert!(trust_state.lock().unwrap().trust.is_none());
            drop(session);
            connection.stop_forwards();
            drop(connection);
            server.abort();
            jump_server.abort();
            echo_task.abort();
            for path in [source, destination, known] {
                std::fs::remove_file(path).unwrap();
            }
            std::fs::remove_dir(dir).unwrap();
        })
        .await
        .unwrap();
    });
}

/// Per-test scratch directory for known_hosts and key material.
fn auth_fixture(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gterminal-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn expand_identity_resolves_tilde_prefix() {
    if let Some(home) = directories::UserDirs::new().map(|d| d.home_dir().to_path_buf()) {
        assert_eq!(
            expand_identity("~/.ssh/id_ed25519"),
            home.join(".ssh/id_ed25519")
        );
        assert_eq!(
            expand_identity(r"  ~\.ssh\id_rsa  "),
            home.join(r".ssh\id_rsa")
        );
    }
    // Anything else is left exactly as typed, trimmed.
    assert_eq!(
        expand_identity(r"  C:\keys\id_rsa  "),
        PathBuf::from(r"C:\keys\id_rsa")
    );
    assert_eq!(expand_identity(""), PathBuf::from(""));
}

#[test]
fn publickey_login_uses_configured_identity_without_password() {
    runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(20), async {
            let host_key = keys::PrivateKey::random(
                &mut ssh_key::rand_core::OsRng,
                ssh_key::Algorithm::Ed25519,
            )
            .unwrap();
            let client_key = keys::PrivateKey::random(
                &mut ssh_key::rand_core::OsRng,
                ssh_key::Algorithm::Ed25519,
            )
            .unwrap();
            let store: Store = Arc::new(Mutex::new(HashMap::new()));
            let (port, server) = test_server(
                host_key.clone(),
                store,
                empty_nodes(),
                vec![client_key.public_key().clone()],
            )
            .await;
            let dir = auth_fixture("gterminal-pubkey");
            let known = dir.join("known_hosts");
            keys::known_hosts::learn_known_hosts_path(
                "127.0.0.1",
                port,
                host_key.public_key(),
                &known,
            )
            .unwrap();
            // Written out in OpenSSH format, the way a key on disk would be.
            let identity = dir.join("id_ed25519");
            let pem = client_key.to_openssh(ssh_key::LineEnding::LF).unwrap();
            std::fs::write(&identity, pem.as_str()).unwrap();
            let profile = RemoteProfile {
                host: "127.0.0.1".into(),
                port,
                user: "test".into(),
                identity: identity.display().to_string(),
                ..Default::default()
            };
            // No password at all: only the key can authenticate this connection.
            let connection = connect_inner(
                profile,
                Credentials::default(),
                Arc::new(Mutex::new(ConnectState::default())),
                Arc::new(|| {}),
                Some(known),
            )
            .await
            .unwrap();
            drop(connection);
            server.abort();
            std::fs::remove_dir_all(dir).unwrap();
        })
        .await
        .unwrap();
    });
}

#[test]
fn unreadable_identity_falls_back_to_password() {
    runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(20), async {
            let host_key = keys::PrivateKey::random(
                &mut ssh_key::rand_core::OsRng,
                ssh_key::Algorithm::Ed25519,
            )
            .unwrap();
            let store: Store = Arc::new(Mutex::new(HashMap::new()));
            let (port, server) = test_server(host_key.clone(), store, empty_nodes(), vec![]).await;
            let dir = auth_fixture("gterminal-fallback");
            let known = dir.join("known_hosts");
            keys::known_hosts::learn_known_hosts_path(
                "127.0.0.1",
                port,
                host_key.public_key(),
                &known,
            )
            .unwrap();
            let profile = RemoteProfile {
                host: "127.0.0.1".into(),
                port,
                user: "test".into(),
                // This used to abort the whole connection instead of letting
                // password authentication proceed.
                identity: dir.join("no-such-key").display().to_string(),
                ..Default::default()
            };
            let credentials = Credentials {
                password: Zeroizing::new("test-password".into()),
                ..Default::default()
            };
            let connection = connect_inner(
                profile,
                credentials,
                Arc::new(Mutex::new(ConnectState::default())),
                Arc::new(|| {}),
                Some(known),
            )
            .await
            .expect("an unreadable identity must not block password login");
            drop(connection);
            server.abort();
            std::fs::remove_dir_all(dir).unwrap();
        })
        .await
        .unwrap();
    });
}

#[test]
fn auth_failure_names_every_method_tried() {
    runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(20), async {
            let host_key = keys::PrivateKey::random(
                &mut ssh_key::rand_core::OsRng,
                ssh_key::Algorithm::Ed25519,
            )
            .unwrap();
            let store: Store = Arc::new(Mutex::new(HashMap::new()));
            let (port, server) = test_server(host_key.clone(), store, empty_nodes(), vec![]).await;
            let dir = auth_fixture("gterminal-diagnostic");
            let known = dir.join("known_hosts");
            keys::known_hosts::learn_known_hosts_path(
                "127.0.0.1",
                port,
                host_key.public_key(),
                &known,
            )
            .unwrap();
            let identity = dir.join("no-such-key");
            let profile = RemoteProfile {
                host: "127.0.0.1".into(),
                port,
                user: "test".into(),
                identity: identity.display().to_string(),
                ..Default::default()
            };
            let credentials = Credentials {
                password: Zeroizing::new("wrong-password".into()),
                ..Default::default()
            };
            let error = match connect_inner(
                profile,
                credentials,
                Arc::new(Mutex::new(ConnectState::default())),
                Arc::new(|| {}),
                Some(known),
            )
            .await
            {
                Ok(_) => panic!("bad credentials must fail"),
                Err(error) => error,
            };
            let message = format!("{error:#}");
            assert!(
                message.contains("no-such-key"),
                "error should name the unusable identity: {message}"
            );
            assert!(
                message.contains("密码认证被服务器拒绝"),
                "error should report the password attempt: {message}"
            );
            server.abort();
            std::fs::remove_dir_all(dir).unwrap();
        })
        .await
        .unwrap();
    });
}

/// Spins up an in-process server over `store`/`nodes` and connects to it.
async fn connected(
    tag: &str,
    store: Store,
    nodes: Nodes,
) -> (Arc<Connection>, PathBuf, JoinHandle<()>) {
    connected_opts(tag, store, nodes, false).await
}

async fn connected_opts(
    tag: &str,
    store: Store,
    nodes: Nodes,
    allow_chown: bool,
) -> (Arc<Connection>, PathBuf, JoinHandle<()>) {
    let host_key =
        keys::PrivateKey::random(&mut ssh_key::rand_core::OsRng, ssh_key::Algorithm::Ed25519)
            .unwrap();
    let (port, server) =
        test_server_opts(host_key.clone(), store, nodes, vec![], allow_chown).await;
    let dir = auth_fixture(tag);
    let known = dir.join("known_hosts");
    keys::known_hosts::learn_known_hosts_path("127.0.0.1", port, host_key.public_key(), &known)
        .unwrap();
    let profile = RemoteProfile {
        host: "127.0.0.1".into(),
        port,
        user: "test".into(),
        ..Default::default()
    };
    let credentials = Credentials {
        password: Zeroizing::new("test-password".into()),
        ..Default::default()
    };
    let connection = connect_inner(
        profile,
        credentials,
        Arc::new(Mutex::new(ConnectState::default())),
        Arc::new(|| {}),
        Some(known),
    )
    .await
    .unwrap();
    (connection, dir, server)
}

fn tree() -> (Store, Nodes) {
    let store: Store = Arc::new(Mutex::new(HashMap::new()));
    let nodes: Nodes = Arc::new(Mutex::new(HashMap::new()));
    {
        let mut files = store.lock().unwrap();
        files.insert("/tree/top.txt".into(), b"a".to_vec());
        files.insert("/tree/sub/deep.txt".into(), b"b".to_vec());
        files.insert("/tree/sub/deeper/leaf.txt".into(), b"c".to_vec());
        files.insert("/keep.txt".into(), b"keep".to_vec());
        let mut n = nodes.lock().unwrap();
        n.insert("/tree".into(), Node::Dir);
        n.insert("/tree/sub".into(), Node::Dir);
        n.insert("/tree/sub/deeper".into(), Node::Dir);
        n.insert("/tree/empty".into(), Node::Dir);
        // A link inside the tree. Following it instead of unlinking it is exactly
        // how a recursive delete walks forever, so it has to be removed as a link.
        n.insert("/tree/loop".into(), Node::Link);
    }
    (store, nodes)
}

#[test]
fn recursive_delete_removes_files_nested_dirs_and_links() {
    runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(20), async {
            let (store, nodes) = tree();
            let (connection, dir, server) =
                connected("gterminal-tree-a", store.clone(), nodes.clone()).await;
            let sftp = connection.sftp("测试").await.unwrap();

            assert!(!directory_is_empty(&sftp, "/tree").await.unwrap());
            assert!(directory_is_empty(&sftp, "/tree/empty").await.unwrap());

            remove(&sftp, "/tree", true).await.unwrap();

            let files = store.lock().unwrap();
            assert!(
                files.keys().all(|k| !k.starts_with("/tree")),
                "files survived the delete: {:?}",
                files.keys().collect::<Vec<_>>()
            );
            assert!(
                files.contains_key("/keep.txt"),
                "the sibling outside the tree was removed too"
            );
            drop(files);
            let n = nodes.lock().unwrap();
            assert!(
                n.keys().all(|k| !k.starts_with("/tree")),
                "directories survived the delete: {:?}",
                n.keys().collect::<Vec<_>>()
            );
            drop(connection);
            server.abort();
            std::fs::remove_dir_all(dir).unwrap();
        })
        .await
        .unwrap();
    });
}

#[test]
fn deleting_a_plain_file_and_an_empty_directory_both_work() {
    runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(20), async {
            let (store, nodes) = tree();
            let (connection, dir, server) =
                connected("gterminal-tree-b", store.clone(), nodes.clone()).await;
            let sftp = connection.sftp("测试").await.unwrap();

            remove(&sftp, "/keep.txt", false).await.unwrap();
            assert!(!store.lock().unwrap().contains_key("/keep.txt"));
            // The empty directory is removed without any recursion.
            remove(&sftp, "/tree/empty", true).await.unwrap();
            assert!(!nodes.lock().unwrap().contains_key("/tree/empty"));
            // A file path can never be removed as a directory.
            assert!(remove(&sftp, "/keep.txt", true).await.is_err());
            drop(connection);
            server.abort();
            std::fs::remove_dir_all(dir).unwrap();
        })
        .await
        .unwrap();
    });
}

/// The guard that keeps the edit round-trip from quietly weakening the
/// no-accidental-overwrite rule every other transfer depends on.
#[test]
fn only_the_edit_round_trip_may_replace_an_existing_target() {
    runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(20), async {
            let store: Store = Arc::new(Mutex::new(HashMap::new()));
            store
                .lock()
                .unwrap()
                .insert("/remote.txt".into(), b"original".to_vec());
            let (connection, dir, server) =
                connected("gterminal-edit-rt", store.clone(), empty_nodes()).await;

            let scratch = auth_fixture("gterminal-edit");
            let local = scratch.join("remote.txt");
            std::fs::write(&local, b"edited").unwrap();

            // A normal upload refuses to replace what is already there.
            let refused = move_one(
                &connection,
                &local,
                "/remote.txt",
                Direction::Upload,
                false,
                "测试",
            )
            .await;
            assert!(refused.is_err(), "a plain upload replaced an existing file");
            assert_eq!(
                store.lock().unwrap().get("/remote.txt").unwrap(),
                b"original"
            );

            // The edit round-trip is allowed to.
            put(&connection, "/remote.txt", &local).await.unwrap();
            assert_eq!(store.lock().unwrap().get("/remote.txt").unwrap(), b"edited");

            drop(connection);
            server.abort();
            std::fs::remove_dir_all(dir).unwrap();
            std::fs::remove_dir_all(scratch).unwrap();
        })
        .await
        .unwrap();
    });
}

fn empty_store() -> Store {
    Arc::new(Mutex::new(HashMap::new()))
}

/// A small local tree: two levels, an empty directory, and three files.
fn local_tree(tag: &str) -> PathBuf {
    let root = auth_fixture(tag);
    std::fs::create_dir_all(root.join("sub").join("deeper")).unwrap();
    std::fs::create_dir_all(root.join("empty")).unwrap();
    std::fs::write(root.join("a.txt"), b"a").unwrap();
    std::fs::write(root.join("sub").join("b.txt"), b"bb").unwrap();
    std::fs::write(root.join("sub").join("deeper").join("c.txt"), b"ccc").unwrap();
    root
}

#[test]
fn local_walk_lists_directories_before_their_children() {
    let root = local_tree("gterminal-walk");
    let (directories, files) = walk_local(&root).unwrap();

    let names: Vec<String> = directories
        .iter()
        .map(|p| relative_to(&root, p, true).unwrap())
        .collect();
    assert!(names.contains(&"sub".to_string()), "{names:?}");
    assert!(names.contains(&"empty".to_string()), "{names:?}");
    // A directory has to appear before its own children, or the mkdir pass would
    // try to create a child whose parent does not exist yet.
    let sub = names.iter().position(|n| n == "sub").unwrap();
    let deeper = names.iter().position(|n| n == "sub/deeper").unwrap();
    assert!(sub < deeper, "sub must come before sub/deeper: {names:?}");

    let mut listed: Vec<String> = files
        .iter()
        .map(|(p, _)| relative_to(&root, p, true).unwrap())
        .collect();
    listed.sort();
    assert_eq!(
        listed,
        ["a.txt", "sub/b.txt", "sub/deeper/c.txt"],
        "the walk must find every file"
    );
    assert_eq!(files.iter().map(|(_, size)| *size).sum::<u64>(), 6);
    std::fs::remove_dir_all(root).unwrap();
}

/// Symlinks are skipped, never followed — following one can leave the tree or
/// loop. Creating a link needs a privilege Windows does not grant by default, so
/// the assertion is skipped when the link cannot be made.
#[test]
fn local_walk_skips_symlinks_when_one_can_be_made() {
    let root = local_tree("gterminal-walk-link");
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_dir(root.join("sub"), root.join("link")).is_ok();
    #[cfg(not(windows))]
    let made = std::os::unix::fs::symlink(root.join("sub"), root.join("link")).is_ok();
    if made {
        let (directories, files) = walk_local(&root).unwrap();
        assert!(
            directories.iter().all(|p| p.file_name().unwrap() != "link"),
            "a directory symlink was descended into"
        );
        assert!(
            files.iter().all(|(p, _)| p.file_name().unwrap() != "link"),
            "a directory symlink was picked up as a file"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn plan_upload_builds_the_remote_tree_and_lists_the_files() {
    runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(20), async {
            let root = local_tree("gterminal-plan-up-tree");
            let store = empty_store();
            let nodes = empty_nodes();
            let (connection, dir, server) =
                connected("gterminal-plan-up", store, nodes.clone()).await;
            let sftp = connection.sftp("测试").await.unwrap();

            let files = plan_upload(&sftp, &root, "/dest").await.unwrap();
            let mut remotes: Vec<_> = files.iter().map(|f| f.remote.clone()).collect();
            remotes.sort();
            assert_eq!(
                remotes,
                ["/dest/a.txt", "/dest/sub/b.txt", "/dest/sub/deeper/c.txt"]
            );
            assert_eq!(files.iter().map(|f| f.size).sum::<u64>(), 6);
            // The walk creates the far-side directories, including the empty one.
            let created = nodes.lock().unwrap();
            for wanted in ["/dest", "/dest/sub", "/dest/sub/deeper", "/dest/empty"] {
                assert!(
                    created.contains_key(wanted),
                    "{wanted} was not created: {created:?}"
                );
            }
            drop(created);

            drop(connection);
            server.abort();
            std::fs::remove_dir_all(root).unwrap();
            std::fs::remove_dir_all(dir).unwrap();
        })
        .await
        .unwrap();
    });
}

#[test]
fn plan_download_builds_the_local_tree_and_lists_the_files() {
    runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(20), async {
            let store = empty_store();
            let nodes = empty_nodes();
            {
                store
                    .lock()
                    .unwrap()
                    .insert("/src/top.txt".into(), b"top".to_vec());
                store
                    .lock()
                    .unwrap()
                    .insert("/src/sub/deep.txt".into(), b"deep".to_vec());
                let mut n = nodes.lock().unwrap();
                n.insert("/src".into(), Node::Dir);
                n.insert("/src/sub".into(), Node::Dir);
                n.insert("/src/empty".into(), Node::Dir);
            }
            let (connection, dir, server) =
                connected("gterminal-plan-down", store, nodes.clone()).await;
            let sftp = connection.sftp("测试").await.unwrap();
            let root = dir.join("downloads");

            let files = plan_download(&sftp, "/src", &root).await.unwrap();
            let mut locals: Vec<String> = files
                .iter()
                // Normalised to `/` so the expectation reads the same on both
                // platforms; the real path keeps the local separator.
                .map(|f| relative_to(&root, &f.local, true).unwrap())
                .collect();
            locals.sort();
            assert_eq!(locals, ["sub/deep.txt", "top.txt"]);
            assert_eq!(files.iter().map(|f| f.size).sum::<u64>(), 7);
            // The walk doubles as the mkdir pass, empty directory included.
            assert!(root.join("sub").is_dir());
            assert!(root.join("empty").is_dir());

            drop(connection);
            server.abort();
            std::fs::remove_dir_all(dir).unwrap();
        })
        .await
        .unwrap();
    });
}

/// Drives one upload through `transfer_file`, the way `Transfer::start` does.
async fn upload_file(
    sftp: &Arc<russh_sftp::client::SftpSession>,
    local: &Path,
    remote: &str,
    control: &TransferControl,
) -> Result<()> {
    let state = Arc::new(Mutex::new(TransferState {
        done: 0,
        total: 0,
        message: String::new(),
        finished: false,
        running: true,
    }));
    let wake: Wake = Arc::new(|| {});
    transfer_file(
        sftp,
        local,
        remote,
        Direction::Upload,
        &state,
        control,
        &wake,
    )
    .await
}

/// A control whose conflicts the test answers itself.
fn live_control(
    batch: u64,
    batch_size: usize,
) -> (
    TransferControl,
    tokio::sync::mpsc::UnboundedReceiver<Conflict>,
) {
    let (conflicts, rx) = tokio::sync::mpsc::unbounded_channel();
    (
        TransferControl {
            pause: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            skipped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            overwrite: false,
            policy: Arc::new(Mutex::new(BatchPolicy::default())),
            conflicts,
            batch_size,
            batch,
        },
        rx,
    )
}

/// A rename has to move the `.part` sibling too. Resolving the destination after
/// those paths were built would leave the partial under the name the user just
/// declined, which is the easiest thing to get wrong here.
#[test]
fn choosing_rename_lands_the_file_under_the_new_name() {
    runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(20), async {
            let store = empty_store();
            store
                .lock()
                .unwrap()
                .insert("/dest.txt".into(), b"existing".to_vec());
            let (connection, dir, server) =
                connected("gterminal-rename", store.clone(), empty_nodes()).await;
            let sftp = connection.sftp("测试").await.unwrap();
            let local = dir.join("dest.txt");
            std::fs::write(&local, b"incoming!!").unwrap();

            let (control, mut rx) = live_control(3, 1);
            let answer = async {
                let conflict = rx.recv().await.expect("a conflict was expected");
                assert_eq!(conflict.name, "dest.txt");
                assert_eq!(conflict.existing, 8, "existing size");
                assert_eq!(conflict.incoming, 10, "incoming size");
                conflict
                    .answer
                    .unwrap()
                    .send(ConflictChoice::Rename)
                    .unwrap();
            };
            let (result, ()) =
                tokio::join!(upload_file(&sftp, &local, "/dest.txt", &control), answer);
            result.unwrap();

            let files = store.lock().unwrap();
            assert_eq!(
                files.get("/dest.txt").unwrap(),
                b"existing",
                "the declined target was replaced anyway"
            );
            assert_eq!(files.get("/dest (1).txt").unwrap(), b"incoming!!");
            assert!(
                !files.contains_key("/dest.txt.gterminal.part"),
                "a partial was left under the declined name"
            );
            assert!(
                !files.contains_key("/dest (1).txt.gterminal.part"),
                "a partial was left next to the committed file"
            );
            drop(files);

            drop(connection);
            server.abort();
            std::fs::remove_dir_all(dir).unwrap();
        })
        .await
        .unwrap();
    });
}

/// "Apply to all" covers the rest of this batch, and a later batch starts fresh.
#[test]
fn apply_to_all_covers_the_batch_and_stops_at_its_edge() {
    runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(20), async {
            let store = empty_store();
            for name in ["a.txt", "b.txt", "c.txt"] {
                store
                    .lock()
                    .unwrap()
                    .insert(format!("/{name}"), b"existing".to_vec());
            }
            let (connection, dir, server) =
                connected("gterminal-batch", store.clone(), empty_nodes()).await;
            let sftp = connection.sftp("测试").await.unwrap();
            for name in ["a.txt", "b.txt", "c.txt"] {
                std::fs::write(dir.join(name), b"incoming!").unwrap();
            }

            let (control, mut rx) = live_control(7, 3);
            let answer = async {
                let conflict = rx.recv().await.expect("the first file must ask");
                conflict
                    .answer
                    .unwrap()
                    .send(ConflictChoice::RenameAll)
                    .unwrap();
            };
            let a = dir.join("a.txt");
            let (first, ()) = tokio::join!(upload_file(&sftp, &a, "/a.txt", &control), answer);
            first.unwrap();
            // The second file of the same batch must not ask again.
            let b = dir.join("b.txt");
            upload_file(&sftp, &b, "/b.txt", &control).await.unwrap();
            assert!(
                rx.try_recv().is_err(),
                "the second file of the batch asked despite an apply-to-all answer"
            );

            // A new batch is a new policy, so it asks afresh.
            let (next, mut next_rx) = live_control(8, 1);
            let answer = async {
                let conflict = next_rx.recv().await.expect("a new batch must ask again");
                conflict
                    .answer
                    .unwrap()
                    .send(ConflictChoice::Rename)
                    .unwrap();
            };
            let c = dir.join("c.txt");
            let (third, ()) = tokio::join!(upload_file(&sftp, &c, "/c.txt", &next), answer);
            third.unwrap();

            let files = store.lock().unwrap();
            // The numbered name goes before the extension, not after the whole name.
            for (name, renamed) in [
                ("a.txt", "/a (1).txt"),
                ("b.txt", "/b (1).txt"),
                ("c.txt", "/c (1).txt"),
            ] {
                assert_eq!(
                    files.get(&format!("/{name}")).unwrap(),
                    b"existing",
                    "{name} was replaced instead of renamed"
                );
                assert!(
                    files.contains_key(renamed),
                    "{name} was not renamed to {renamed}"
                );
            }
            drop(files);

            drop(connection);
            server.abort();
            std::fs::remove_dir_all(dir).unwrap();
        })
        .await
        .unwrap();
    });
}

/// Skipping settles the entry rather than leaving it resumable, and cancelling the
/// rest marks the whole batch so the files behind it never start.
#[test]
fn skipping_and_cancelling_settle_without_offering_to_resume() {
    runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(20), async {
            let store = empty_store();
            store
                .lock()
                .unwrap()
                .insert("/a.txt".into(), b"existing".to_vec());
            let (connection, dir, server) =
                connected("gterminal-skip", store.clone(), empty_nodes()).await;
            let sftp = connection.sftp("测试").await.unwrap();
            let source = dir.join("a.txt");
            std::fs::write(&source, b"incoming!").unwrap();

            let (skip, mut rx) = live_control(9, 2);
            let answer = async {
                let conflict = rx.recv().await.expect("a conflict was expected");
                conflict.answer.unwrap().send(ConflictChoice::Skip).unwrap();
            };
            let (result, ()) = tokio::join!(upload_file(&sftp, &source, "/a.txt", &skip), answer);
            let error = format!("{:#}", result.unwrap_err());
            assert!(error.contains("已跳过"), "{error}");
            assert!(
                skip.skipped.load(std::sync::atomic::Ordering::Acquire),
                "the entry must be marked skipped so the queue does not offer to resume"
            );
            assert_eq!(
                store.lock().unwrap().get("/a.txt").unwrap(),
                b"existing",
                "skipping must not touch the target"
            );

            let (cancel, mut rx) = live_control(10, 2);
            let answer = async {
                let conflict = rx.recv().await.expect("a conflict was expected");
                conflict
                    .answer
                    .unwrap()
                    .send(ConflictChoice::CancelRemaining)
                    .unwrap();
            };
            let (result, ()) = tokio::join!(upload_file(&sftp, &source, "/a.txt", &cancel), answer);
            assert!(result.is_err());
            assert!(
                cancel.policy.lock().unwrap().cancelled(),
                "cancelling must mark the batch so the files behind it never start"
            );

            drop(connection);
            server.abort();
            std::fs::remove_dir_all(dir).unwrap();
        })
        .await
        .unwrap();
    });
}

/// A tree with two levels, for the permission tests.
fn perm_tree() -> (Store, Nodes) {
    let store: Store = Arc::new(Mutex::new(HashMap::new()));
    let nodes: Nodes = Arc::new(Mutex::new(HashMap::new()));
    {
        let mut files = store.lock().unwrap();
        files.insert("/perm/top.txt".into(), b"a".to_vec());
        files.insert("/perm/sub/deep.txt".into(), b"b".to_vec());
        let mut n = nodes.lock().unwrap();
        n.insert("/perm".into(), Node::Dir);
        n.insert("/perm/sub".into(), Node::Dir);
        // A link inside the tree: it must be listed but never descended into.
        n.insert("/perm/loop".into(), Node::Link);
    }
    (store, nodes)
}

#[test]
fn walking_lists_every_path_without_following_links() {
    runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(20), async {
            let (store, nodes) = perm_tree();
            let (connection, dir, server) = connected("gterminal-walk-remote", store, nodes).await;
            let sftp = connection.sftp("测试").await.unwrap();

            let mut paths = walk_remote(&sftp, "/perm").await.unwrap();
            paths.sort();
            assert_eq!(
                paths,
                [
                    "/perm",
                    "/perm/loop",
                    "/perm/sub",
                    "/perm/sub/deep.txt",
                    "/perm/top.txt"
                ],
                "every path once, and the link not descended into"
            );
            // The parent has to come first, so a caller can act on it before its
            // children.
            let raw = walk_remote(&sftp, "/perm").await.unwrap();
            let position = |needle: &str| raw.iter().position(|p| p == needle).unwrap();
            assert!(position("/perm") < position("/perm/sub"));
            assert!(position("/perm/sub") < position("/perm/sub/deep.txt"));

            drop(connection);
            server.abort();
            std::fs::remove_dir_all(dir).unwrap();
        })
        .await
        .unwrap();
    });
}

/// The mode change has to reach the server and be visible afterwards.
///
/// This also pins the request's *contents*: the server here refuses any setstat
/// carrying an owner, so a chmod that also asked to change ownership — which is
/// what spreading `FileAttributes::default()` into the request would do, since its
/// default is uid 0 rather than none — fails here rather than on a real server.
#[test]
fn setting_permissions_changes_the_mode_the_server_reports() {
    runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(20), async {
            let (store, nodes) = perm_tree();
            let (connection, dir, server) = connected("gterminal-chmod", store, nodes).await;
            let sftp = connection.sftp("测试").await.unwrap();

            assert_eq!(
                sftp.metadata("/perm/top.txt").await.unwrap().permissions,
                Some(0o100644)
            );
            set_permissions(&sftp, "/perm/top.txt", 0o100755)
                .await
                .expect("a chmod must not ask for anything but the mode");
            assert_eq!(
                sftp.metadata("/perm/top.txt").await.unwrap().permissions,
                Some(0o100755),
                "the new mode was not read back"
            );
            // A directory takes one too, and the type bits are the server's to set.
            set_permissions(&sftp, "/perm/sub", 0o040700).await.unwrap();
            assert_eq!(
                sftp.metadata("/perm/sub").await.unwrap().permissions,
                Some(0o040700)
            );

            drop(connection);
            server.abort();
            std::fs::remove_dir_all(dir).unwrap();
        })
        .await
        .unwrap();
    });
}

/// The whole reason chmod and chown are sent as two requests rather than one:
/// a server refuses the ownership half for a non-root user, and sending them
/// together would take the mode change down with it.
#[test]
fn a_refused_chown_does_not_take_the_mode_change_with_it() {
    runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(20), async {
            let (store, nodes) = perm_tree();
            // `false` makes the server refuse any setstat carrying an owner.
            let (connection, dir, server) =
                connected_opts("gterminal-chown-refused", store, nodes, false).await;
            let sftp = connection.sftp("测试").await.unwrap();

            set_permissions(&sftp, "/perm/top.txt", 0o100600)
                .await
                .unwrap();
            let refused = set_owner(&sftp, "/perm/top.txt", Some(1000), None).await;
            assert!(refused.is_err(), "the server was supposed to refuse this");
            assert_eq!(
                sftp.metadata("/perm/top.txt").await.unwrap().permissions,
                Some(0o100600),
                "the mode change was undone by the refused ownership change"
            );

            drop(connection);
            server.abort();
            std::fs::remove_dir_all(dir).unwrap();
        })
        .await
        .unwrap();
    });
}

/// And where the server does permit it, ownership lands.
#[test]
fn setting_an_owner_reaches_the_server_when_it_is_allowed() {
    runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(20), async {
            let (store, nodes) = perm_tree();
            let (connection, dir, server) =
                connected_opts("gterminal-chown-allowed", store, nodes, true).await;
            let sftp = connection.sftp("测试").await.unwrap();

            set_owner(&sftp, "/perm/top.txt", Some(1000), Some(100))
                .await
                .unwrap();
            let metadata = sftp.metadata("/perm/top.txt").await.unwrap();
            assert_eq!(metadata.uid, Some(1000));
            assert_eq!(metadata.gid, Some(100));

            drop(connection);
            server.abort();
            std::fs::remove_dir_all(dir).unwrap();
        })
        .await
        .unwrap();
    });
}

/// A numeric owner skips the lookup entirely — which is what makes the dialog
/// usable on a server with no shell, and is also the path that needs no quoting.
#[test]
fn a_numeric_owner_is_used_as_is() {
    runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(20), async {
            let (store, nodes) = perm_tree();
            let (connection, dir, server) = connected("gterminal-uid-numeric", store, nodes).await;
            // No server call is made for a number, so a connection that cannot run
            // commands at all still resolves it.
            assert_eq!(resolve_uid(&connection, "0").await.unwrap(), 0);
            assert_eq!(
                resolve_uid(&connection, "  1000  ").await.unwrap(),
                1000,
                "surrounding space must not defeat the shortcut"
            );
            assert_eq!(resolve_gid(&connection, "100").await.unwrap(), 100);

            drop(connection);
            server.abort();
            std::fs::remove_dir_all(dir).unwrap();
        })
        .await
        .unwrap();
    });
}
