//! Local shells and serial ports: spawning, respawning and the screen they
//! write into.

use super::*;

impl Session {
    /// The screen a new session writes into: the previous one when reconnecting,
    /// so its scrollback survives the drop, otherwise a fresh bounded screen.
    pub(super) fn screen(
        previous: Option<Arc<Mutex<Terminal>>>,
        rows: u16,
        cols: u16,
        scrollback: usize,
    ) -> Arc<Mutex<Terminal>> {
        match previous {
            Some(terminal) => {
                terminal
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .recycle(rows, cols);
                terminal
            }
            None => Arc::new(Mutex::new(Terminal::new(rows, cols, scrollback))),
        }
    }

    /// Opens a serial device and wires it straight into the terminal. There is
    /// no child process: the session lives until the device is unplugged, the
    /// port errors, or the pane closes.
    pub fn from_serial(
        profile: SerialProfile,
        scrollback: usize,
        wake: Arc<dyn Fn() + Send + Sync>,
        previous: Option<Arc<Mutex<Terminal>>>,
    ) -> Result<Self> {
        serial_log!("连接串口 {} @ {} bps", profile.port.trim(), profile.baud);
        let requested = profile.port.trim().to_string();
        let name = match crate::serial::resolve(&profile.port) {
            Ok(name) => name,
            Err(e) => {
                serial_log!("串口解析失败: {e:#}");
                return Err(e);
            }
        };
        if !name.eq_ignore_ascii_case(&requested) {
            serial_log!("auto 解析为 {name}");
        }
        if crate::serial::held(&name) {
            let error = anyhow::anyhow!("串口 {name} 已被本程序占用，请先关闭对应的会话");
            serial_log!("{error}");
            return Err(error);
        }
        let port =
            match crate::serial::open_bounded(&name, profile.baud, crate::serial::OPEN_TIMEOUT) {
                Ok(port) => port,
                Err(e) => {
                    serial_log!("打开 {name} 失败: {e:#}");
                    return Err(e);
                }
            };
        serial_log!("已连接 {name} @ {} bps", profile.baud);
        let port = Arc::new(port);
        let guard = crate::serial::occupy(&name);
        let size = (30u16, 100u16);
        let terminal = Self::screen(previous, size.0, size.1, scrollback);
        let status = Arc::new(Mutex::new(Status::default()));
        let traffic = Arc::new(Traffic::default());
        let stop = Arc::new(AtomicBool::new(false));
        let (input, receiver) = mpsc::sync_channel::<Control>(64);

        let output_terminal = terminal.clone();
        let output_status = status.clone();
        let reader_traffic = traffic.clone();
        let output_wake = wake.clone();
        let reply_sender = input.clone();
        let reader_port = port.clone();
        let reader_stop = stop.clone();
        let reader = thread::spawn(move || {
            let mut bytes = [0u8; 16 * 1024];
            loop {
                if reader_stop.load(Ordering::Acquire) {
                    break;
                }
                match reader_port.read(&mut bytes) {
                    // A zero-length read is the device going away.
                    Ok(0) => {
                        report_end(&output_status, &output_terminal, "串口设备已断开".into());
                        break;
                    }
                    Ok(n) => {
                        reader_traffic.add_down(n);
                        let replies = {
                            let mut terminal =
                                output_terminal.lock().unwrap_or_else(|e| e.into_inner());
                            if terminal.process(&bytes[..n]) {
                                reader_traffic
                                    .bell
                                    .store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                            std::mem::take(&mut terminal.parser.callbacks_mut().replies)
                        };
                        if !replies.is_empty() {
                            let _ = reply_sender.try_send(Control::Write(replies));
                        }
                        output_wake();
                    }
                    // The read timeout is how a shutdown request is noticed.
                    Err(e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        report_end(
                            &output_status,
                            &output_terminal,
                            format!("串口连接中断: {e}"),
                        );
                        break;
                    }
                }
            }
            output_status.lock().unwrap().eof = true;
            output_wake();
        });

        let control_stop = stop.clone();
        let control_terminal = terminal.clone();
        let control_traffic = traffic.clone();
        let writer_port = port.clone();
        let control = thread::spawn(move || {
            // Bytes the device has not accepted yet. A serial write can time
            // out (Windows reports os error 121) when its buffer is full, which
            // is normal under flow control; what was accepted is dropped from
            // the front so retrying can never send a byte twice, and the rest
            // waits for the next tick.
            let mut pending: Vec<u8> = Vec::new();
            while !control_stop.load(Ordering::Acquire) {
                if !pending.is_empty() {
                    match writer_port.write(&pending) {
                        Ok(0) => {}
                        Ok(n) => {
                            control_traffic.add_up(n);
                            pending.drain(..n);
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                        Err(e) => {
                            // A real failure, not a full buffer. The reader
                            // reports the disconnect; the queue is dropped so
                            // the failure cannot spin forever.
                            serial_log!("写入失败，丢弃 {} 字节: {e}", pending.len());
                            pending.clear();
                        }
                    }
                }
                match receiver.recv_timeout(Duration::from_millis(20)) {
                    Ok(Control::Write(bytes)) => pending.extend_from_slice(&bytes),
                    // The device has no size, but the terminal still has to
                    // follow the pane. No ZMODEM channel is ever opened here.
                    Ok(Control::Resize(size)) => control_terminal
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .resize(size.rows, size.cols),
                    Ok(Control::ZmodemBridge { .. }) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        });

        Ok(Self {
            terminal,
            status,
            traffic,
            kind: SessionKind::Serial(profile),
            pid: None,
            size,
            input,
            stop,
            killer: None,
            remote: None,
            detached: false,
            pending: false,
            serial_workers: vec![reader, control],
            _serial_guard: Some(guard),
        })
    }

    pub fn spawn(
        kind: SessionKind,
        scrollback: usize,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        Self::spawn_reusing(kind, scrollback, wake, None)
    }

    /// Like [`Self::spawn`], but a reconnect can hand over the screen of the
    /// session it replaces so the output history stays visible.
    pub fn spawn_reusing(
        kind: SessionKind,
        scrollback: usize,
        wake: Arc<dyn Fn() + Send + Sync>,
        previous: Option<Arc<Mutex<Terminal>>>,
    ) -> Result<Self> {
        if let SessionKind::Serial(profile) = &kind {
            return Self::from_serial(profile.clone(), scrollback, wake, previous);
        }
        let size = PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pty_hint = if cfg!(windows) {
            "无法创建系统 PTY（Windows 需要 10 1809 或更新版本）"
        } else {
            "无法创建系统 PTY"
        };
        let pair = native_pty_system().openpty(size).context(pty_hint)?;
        let mut reader = pair.master.try_clone_reader()?;
        let mut writer = pair.master.take_writer()?;
        let mut child = pair.slave.spawn_command(kind.command()?).with_context(|| {
            format!(
                "无法启动 {}，请检查程序是否已安装并在 PATH 中",
                kind.label()
            )
        })?;
        let pid = child.process_id();
        let killer = child.clone_killer();
        drop(pair.slave);
        let terminal = Self::screen(previous, size.rows, size.cols, scrollback);
        let status = Arc::new(Mutex::new(Status::default()));
        let traffic = Arc::new(Traffic::default());
        let stop = Arc::new(AtomicBool::new(false));
        // Bounded input; output is parsed immediately into a bounded screen on a worker.
        let (input, receiver) = mpsc::sync_channel::<Control>(64);
        let output_terminal = terminal.clone();
        let output_status = status.clone();
        let output_traffic = traffic.clone();
        let output_wake = wake.clone();
        let reply_sender = input.clone();
        thread::spawn(move || {
            let mut bytes = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(n) => {
                        output_traffic.add_down(n);
                        let replies = {
                            let mut terminal =
                                output_terminal.lock().unwrap_or_else(|e| e.into_inner());
                            if terminal.process(&bytes[..n]) {
                                output_traffic
                                    .bell
                                    .store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                            std::mem::take(&mut terminal.parser.callbacks_mut().replies)
                        };
                        if !replies.is_empty() {
                            let _ = reply_sender.try_send(Control::Write(replies));
                        }
                        output_wake();
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        // Unix PTYs may report EIO on a normal slave close.
                        if e.raw_os_error() != Some(5) {
                            report_end(&output_status, &output_terminal, format!("连接中断: {e}"));
                        }
                        break;
                    }
                }
            }
            output_status.lock().unwrap().eof = true;
            output_wake();
        });
        let control_stop = stop.clone();
        let control_terminal = terminal.clone();
        let control_traffic = traffic.clone();
        let control_wake = wake.clone();
        thread::spawn(move || {
            let master = pair.master;
            while !control_stop.load(Ordering::Acquire) {
                match receiver.recv_timeout(Duration::from_millis(100)) {
                    Ok(control) => {
                        let result = match control {
                            Control::Write(bytes) => {
                                let written = writer
                                    .write_all(&bytes)
                                    .and_then(|_| writer.flush())
                                    .map_err(anyhow::Error::from);
                                if written.is_ok() {
                                    control_traffic.add_up(bytes.len());
                                }
                                written
                            }
                            Control::Resize(size) => master.resize(size).map(|_| {
                                control_terminal
                                    .lock()
                                    .unwrap()
                                    .resize(size.rows, size.cols)
                            }),
                            Control::ZmodemBridge { .. } => Ok(()),
                        };
                        if let Err(e) = result {
                            // Writing to a pty whose child has gone is not a
                            // disconnect either: the reader and the waiter are
                            // what report the end of the session.
                            session_log!("输入暂时失败（将重试）: {e}");
                            control_wake();
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
            drop(writer);
            drop(master);
        });
        let wait_status = status.clone();
        let wait_terminal = terminal.clone();
        thread::spawn(move || {
            match child.wait() {
                Ok(exit) => {
                    let code = exit.exit_code();
                    wait_status
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .exit_code = Some(code);
                    if code != 0 {
                        wait_terminal
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .note(&format!("进程已退出（退出码 {code}）"));
                    }
                }
                Err(e) => report_end(&wait_status, &wait_terminal, format!("进程等待失败: {e}")),
            }
            wake();
        });
        Ok(Self {
            terminal,
            status,
            traffic,
            kind,
            pid,
            size: (size.rows, size.cols),
            input,
            stop,
            killer: Some(killer),
            remote: None,
            detached: false,
            pending: false,
            serial_workers: Vec::new(),
            _serial_guard: None,
        })
    }
}
