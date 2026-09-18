//! SSH sessions: the channel, its pumps and the console mirrored into it.

use super::*;

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
        let (input, receiver) = mpsc::sync_channel::<Control>(64);
        let (out, state, counted, stopped, connection_task) = (
            terminal.clone(),
            status.clone(),
            traffic.clone(),
            stop.clone(),
            connection.clone(),
        );
        crate::remote::runtime().spawn(async move {
            let result: Result<()> = async {
                let receiver = receiver;
                let mut channel = connection_task.handle.channel_open_session().await?;
                channel.request_pty(true, "xterm-256color", 100, 30, 0, 0, &[]).await?;
                channel.request_shell(true).await?;
                let mut tick = tokio::time::interval(Duration::from_millis(10));
                let mut bridge: Option<ZmodemBridge> = None;
                loop {
                    tokio::select! {
                        _ = tick.tick() => {
                            if stopped.load(Ordering::Acquire) { channel.close().await?; break; }
                            if let Some((_, frames)) = bridge.as_mut() {
                                loop {
                                    match frames.try_recv() {
                                        Ok(data) => channel.data(&data[..]).await?,
                                        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                                            // The transfer task finished; resume the shell.
                                            bridge = None;
                                            break;
                                        }
                                        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                                    }
                                }
                            }
                            for _ in 0..16 {
                                match receiver.try_recv() {
                                    Ok(Control::Write(bytes)) => {
                                        // Keystrokes would corrupt an in-flight ZMODEM stream.
                                        if !bridge.is_some() {
                                            channel.data(&bytes[..]).await?;
                                            counted.add_up(bytes.len());
                                        }
                                    }
                                    Ok(Control::Resize(size)) => { channel.window_change(size.cols as u32, size.rows as u32, 0, 0).await?; out.lock().unwrap().resize(size.rows, size.cols); }
                                    Ok(Control::ZmodemBridge { to_transfer, from_transfer }) => { bridge = Some((to_transfer, from_transfer)); }
                                    Err(_) => break,
                                }
                            }
                        }
                        message = channel.wait() => match message {
                            Some(russh::ChannelMsg::Data { data }) | Some(russh::ChannelMsg::ExtendedData { data, .. }) => {
                                if let Some((to_transfer, _)) = bridge.as_ref() {
                                    // While a ZMODEM transfer owns the channel its
                                    // binary stream must bypass the VT parser.
                                    if to_transfer.send(data.to_vec()).is_err() {
                                        bridge = None;
                                    }
                                } else {
                                    counted.add_down(data.len());
                                    let replies = { let mut t=out.lock().unwrap(); if t.process(&data) { counted.bell.store(true, Ordering::Relaxed); } std::mem::take(&mut t.parser.callbacks_mut().replies) };
                                    if !replies.is_empty() { channel.data(&replies[..]).await?; }
                                    wake();
                                }
                            }
                            Some(russh::ChannelMsg::ExitStatus { exit_status }) => { state.lock().unwrap().exit_code=Some(exit_status); wake(); }
                            Some(russh::ChannelMsg::Close) | None => break,
                            _ => {}
                        }
                    }
                }
                Ok(())
            }.await;
            state.lock().unwrap_or_else(|e| e.into_inner()).eof = true;
            match result {
                Err(e) => report_end(&state, &out, format!("{e:#}")),
                Ok(()) => {
                    let finished = state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .exit_code
                        .is_some();
                    if !finished {
                        if stopped.load(Ordering::Acquire) {
                            // A deliberate disconnect closes the channel itself
                            // (see the tick above), so a missing exit status is
                            // expected here — reporting it would put a bogus
                            // error on a screen the user just chose to leave.
                            state.lock().unwrap_or_else(|e| e.into_inner()).exit_code = Some(0);
                        } else {
                            // Closed without an exit status: the transport
                            // dropped rather than the shell exiting on its own.
                            state.lock().unwrap_or_else(|e| e.into_inner()).exit_code = Some(255);
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
            input,
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
