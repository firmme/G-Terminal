use super::*;
use russh::{
    Channel, ChannelId,
    server::{self, Msg},
};
use russh_sftp::protocol::*;
use std::collections::HashMap;
type Store = Arc<Mutex<HashMap<String, Vec<u8>>>>;

struct TestSsh {
    channels: HashMap<ChannelId, Channel<Msg>>,
    store: Store,
    shells: std::collections::HashSet<ChannelId>,
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
        tokio::spawn(async move {
            russh_sftp::server::run(channel.into_stream(), TestSftp { store, read: false }).await;
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
        Ok(Handle { id, handle: path })
    }
    async fn readdir(&mut self, id: u32, _: String) -> std::result::Result<Name, StatusCode> {
        if self.read {
            return Err(StatusCode::Eof);
        }
        self.read = true;
        Ok(Name {
            id,
            files: self
                .store
                .lock()
                .unwrap()
                .iter()
                .map(|(name, data)| {
                    File::new(
                        name.trim_start_matches('/'),
                        FileAttributes {
                            size: Some(data.len() as u64),
                            permissions: Some(0o100644),
                            ..Default::default()
                        },
                    )
                })
                .collect(),
        })
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
        let s = self.store.lock().unwrap();
        let d = s.get(&path).ok_or(StatusCode::NoSuchFile)?;
        Ok(Attrs {
            id,
            attrs: FileAttributes {
                size: Some(d.len() as u64),
                permissions: Some(0o100644),
                ..Default::default()
            },
        })
    }
    async fn lstat(&mut self, id: u32, path: String) -> std::result::Result<Attrs, StatusCode> {
        self.stat(id, path).await
    }
    async fn fstat(&mut self, id: u32, path: String) -> std::result::Result<Attrs, StatusCode> {
        self.stat(id, path).await
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

async fn test_server(key: keys::PrivateKey, store: Store) -> (u16, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = Arc::new(server::Config {
        keys: vec![key],
        auth_rejection_time: Duration::ZERO,
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    });
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let config = config.clone();
            let store = store.clone();
            tokio::spawn(async move {
                let running = server::run_stream(
                    config,
                    stream,
                    TestSsh {
                        channels: HashMap::new(),
                        store,
                        shells: std::collections::HashSet::new(),
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
            let (port, server) = test_server(key.clone(), store.clone()).await;
            let (jump_port, jump_server) = test_server(key.clone(), store.clone()).await;
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
            let pause = std::sync::atomic::AtomicBool::new(false);
            let wake: Wake = Arc::new(|| {});
            transfer_file(
                &connection,
                &source,
                "/data.bin",
                Direction::Upload,
                &state,
                &pause,
                &wake,
            )
            .await
            .unwrap();
            assert_eq!(store.lock().unwrap().get("/data.bin").unwrap(), &data);
            let part = PathBuf::from(format!("{}.gterminal.part", destination.display()));
            std::fs::write(&part, &data[..32000]).unwrap();
            transfer_file(
                &connection,
                &destination,
                "/data.bin",
                Direction::Download,
                &state,
                &pause,
                &wake,
            )
            .await
            .unwrap();
            assert_eq!(std::fs::read(&destination).unwrap(), data);
            assert!(
                transfer_file(
                    &connection,
                    &source,
                    "/data.bin",
                    Direction::Upload,
                    &state,
                    &pause,
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
                    &connection,
                    &source,
                    "/wrong.bin",
                    Direction::Upload,
                    &state,
                    &pause,
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
