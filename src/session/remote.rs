//! SSH sessions: the channel, its pumps and the console mirrored into it.

use super::*;
use crate::LockExt;

impl Session {
    pub fn from_remote(
        connection: Arc<crate::remote::Connection>,
        scrollback: usize,
        wake: crate::remote::Wake,
        previous: Option<Arc<Mutex<Terminal>>>,
    ) -> Self {
        let terminal = Self::screen(previous, 30, 100, scrollback);
        let status = Arc::new(Mutex::new(Status::default()));
        let traffic = Arc::new(Traffic::default());
        let stop = Arc::new(AtomicBool::new(false));
        let (input, mut receiver) = tokio::sync::mpsc::channel::<Control>(64);
        let (out, state, counted, stopped, connection_task) = (
            terminal.clone(),
            status.clone(),
            traffic.clone(),
            stop.clone(),
            connection.clone(),
        );
        crate::remote::runtime().spawn(async move {
            let result: Result<()> = async {
                let mut channel = connection_task.handle.channel_open_session().await?;
                channel.request_pty(true, "xterm-256color", 100, 30, 0, 0, &[]).await?;
                channel.request_shell(true).await?;
                let mut bridge: Option<ZmodemBridge> = None;
                // Event driven: the old 10 ms tick woke every idle SSH session
                // 100 times a second and delayed each keystroke by up to a tick.
                // Dropping the session drops `input`, which ends `recv` here.
                loop {
                    tokio::select! {
                        control = receiver.recv() => match control {
                            Some(Control::Write(bytes)) => {
                                // Keystrokes would corrupt an in-flight ZMODEM stream.
                                if bridge.is_none() {
                                    channel.data(&bytes[..]).await?;
                                    counted.add_up(bytes.len());
                                }
                            }
                            Some(Control::Resize(size)) => {
                                channel.window_change(size.cols as u32, size.rows as u32, 0, 0).await?;
                                out.locked().resize(size.rows, size.cols);
                            }
                            Some(Control::ZmodemBridge { to_transfer, from_transfer }) => {
                                bridge = Some((to_transfer, from_transfer));
                            }
                            None => {
                                channel.close().await?;
                                break;
                            }
                        },
                        frame = async { bridge.as_mut().expect("guarded").1.recv().await }, if bridge.is_some() => match frame {
                            Some(data) => channel.data(&data[..]).await?,
                            // The transfer task finished; resume the shell.
                            None => bridge = None,
                        },
                        message = channel.wait() => match message {
                            Some(russh::ChannelMsg::Data { data }) | Some(russh::ChannelMsg::ExtendedData { data, .. }) => {
                                if let Some((to_transfer, _)) = bridge.as_ref() {
                                    // While a ZMODEM transfer owns the channel its
                                    // binary stream must bypass the VT parser.
                                    // Bounded: a slow transfer holds the channel
                                    // back instead of buffering without limit.
                                    if to_transfer.send(data.to_vec()).await.is_err() {
                                        bridge = None;
                                    }
                                } else {
                                    let replies = feed(&out, &counted, &data);
                                    if !replies.is_empty() {
                                        channel.data(&replies[..]).await?;
                                    }
                                    wake();
                                }
                            }
                            Some(russh::ChannelMsg::ExitStatus { exit_status }) => { state.locked().exit_code=Some(exit_status); wake(); }
                            Some(russh::ChannelMsg::Close) | None => break,
                            _ => {}
                        }
                    }
                }
                Ok(())
            }.await;
            state.locked().eof = true;
            match result {
                Err(e) => report_end(&state, &out, format!("{e:#}")),
                Ok(()) => {
                    let finished = state
                        .locked()
                        .exit_code
                        .is_some();
                    if !finished {
                        if stopped.load(Ordering::Acquire) {
                            // A deliberate disconnect closes the channel itself
                            // (see the tick above), so a missing exit status is
                            // expected here — reporting it would put a bogus
                            // error on a screen the user just chose to leave.
                            state.locked().exit_code = Some(0);
                        } else {
                            // Closed without an exit status: the transport
                            // dropped rather than the shell exiting on its own.
                            state.locked().exit_code = Some(255);
                            report_end(&state, &out, "连接已断开（未收到退出状态）".into());
                        }
                    }
                }
            }
            wake();
        });
        Self {
            terminal,
            status,
            traffic,
            kind: SessionKind::Ssh(connection.profile.clone()),
            pid: None,
            size: (30, 100),
            input: Input::Async(input),
            stop,
            killer: None,
            remote: Some(connection),
            detached: false,
            pending: false,
            serial_workers: Vec::new(),
            _serial_guard: None,
        }
    }
}
