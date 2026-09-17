use crate::{
    config::{RemoteProfile, SerialProfile},
    terminal::Terminal,
};
use anyhow::{Context, Result, bail};
use portable_pty::{ChildKiller, CommandBuilder, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::Duration,
};

/// Console-only diagnostics for serial connections. A release build runs under
/// the GUI subsystem with no stderr, and `eprintln!` panics when its write
/// fails, so the result is discarded here.
macro_rules! serial_log {
    ($($arg:tt)*) => {{
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), "[串口] {}", format_args!($($arg)*));
    }};
}

/// The same, for a session that is neither local PTY nor serial specific.
macro_rules! session_log {
    ($($arg:tt)*) => {{
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), "[会话] {}", format_args!($($arg)*));
    }};
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum SessionKind {
    Local(String),
    Serial(SerialProfile),
    Ssh(RemoteProfile),
    Sftp(RemoteProfile),
}

impl SessionKind {
    pub fn label(&self) -> String {
        match self {
            Self::Local(shell) => match shell.as_str() {
                "powershell" => "PowerShell".into(),
                "pwsh" => "PowerShell 7".into(),
                "cmd" => "Command Prompt".into(),
                "wsl" => "WSL".into(),
                _ => "Shell".into(),
            },
            Self::Serial(p) => p.label(),
            Self::Ssh(p) => p.label(),
            Self::Sftp(p) => format!("SFTP · {}", p.label()),
        }
    }

    pub fn command(&self) -> Result<CommandBuilder> {
        let mut cmd = match self {
            Self::Serial(profile) => bail!("串口会话不通过进程启动：{}", profile.label()),
            Self::Local(shell) => {
                #[cfg(windows)]
                {
                    match shell.as_str() {
                        "powershell" => {
                            let mut c = CommandBuilder::new("powershell.exe");
                            c.arg("-NoLogo");
                            c
                        }
                        "pwsh" => {
                            let mut c = CommandBuilder::new("pwsh.exe");
                            c.arg("-NoLogo");
                            c
                        }
                        "cmd" => CommandBuilder::new("cmd.exe"),
                        "wsl" => CommandBuilder::new("wsl.exe"),
                        _ => bail!("未知 Shell: {shell}"),
                    }
                }
                #[cfg(not(windows))]
                {
                    let _ = shell;
                    CommandBuilder::new(std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()))
                }
            }
            Self::Ssh(profile) | Self::Sftp(profile) => {
                profile.validate()?;
                let sftp = matches!(self, Self::Sftp(_));
                let mut c = CommandBuilder::new(if sftp { "sftp" } else { "ssh" });
                if !sftp {
                    c.arg("-tt");
                }
                c.arg(if sftp { "-P" } else { "-p" });
                c.arg(profile.port.to_string());
                c.args([
                    "-o",
                    "ServerAliveInterval=30",
                    "-o",
                    "ServerAliveCountMax=3",
                ]);
                if !profile.identity.trim().is_empty() {
                    c.arg("-i");
                    c.arg(&profile.identity);
                }
                // Arguments are passed directly to OpenSSH, never interpolated into a shell.
                c.arg("--");
                c.arg(profile.destination());
                c
            }
        };
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "G-Terminal");
        if let Some(home) = directories::UserDirs::new().map(|d| PathBuf::from(d.home_dir())) {
            cmd.cwd(home);
        }
        Ok(cmd)
    }
}

enum Control {
    Write(Vec<u8>),
    Resize(PtySize),
    /// Route raw session bytes through a terminal-initiated ZMODEM transfer.
    ZmodemBridge {
        to_transfer: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
        from_transfer: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    },
}

type ZmodemBridge = (
    tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
);

/// A ZMODEM endpoint wired into a live session channel: data arriving from the
/// remote host is read here, and frames written here are sent back to it.
pub struct BridgeStream {
    rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    buffer: Vec<u8>,
    position: usize,
}

impl tokio::io::AsyncRead for BridgeStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        loop {
            if self.position < self.buffer.len() {
                let n = (self.buffer.len() - self.position).min(buf.remaining());
                buf.put_slice(&self.buffer[self.position..self.position + n]);
                self.position += n;
                if self.position == self.buffer.len() {
                    self.buffer.clear();
                    self.position = 0;
                }
                return std::task::Poll::Ready(Ok(()));
            }
            match self.rx.poll_recv(cx) {
                std::task::Poll::Ready(Some(chunk)) => {
                    self.buffer = chunk;
                    self.position = 0;
                }
                std::task::Poll::Ready(None) => return std::task::Poll::Ready(Ok(())),
                std::task::Poll::Pending => return std::task::Poll::Pending,
            }
        }
    }
}

impl tokio::io::AsyncWrite for BridgeStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        if self.tx.send(buf.to_vec()).is_err() {
            return std::task::Poll::Ready(Err(std::io::Error::from(
                std::io::ErrorKind::BrokenPipe,
            )));
        }
        std::task::Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionStatus {
    /// A saved pane that has not been connected yet.
    Detached,
    /// A live shell or SSH channel.
    Live,
    /// Ended on its own, or dropped without a clean exit.
    Lost,
}

impl SessionStatus {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Detached => "未连接",
            Self::Live => "已连接",
            Self::Lost => "异常断开",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub exit_code: Option<u32>,
    pub error: Option<String>,
    pub eof: bool,
}

/// Byte counters a live session keeps so the status bar can show rates. Shared
/// by the workers and read by the UI, hence atomics rather than a lock.
#[derive(Default)]
pub struct Traffic {
    /// Bytes sent to the session (keystrokes, pastes, protocol replies).
    pub up: std::sync::atomic::AtomicU64,
    /// Bytes received from the session.
    pub down: std::sync::atomic::AtomicU64,
}

impl Traffic {
    fn add_up(&self, bytes: usize) {
        self.up
            .fetch_add(bytes as u64, std::sync::atomic::Ordering::Relaxed);
    }
    fn add_down(&self, bytes: usize) {
        self.down
            .fetch_add(bytes as u64, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Records why a session ended on the session's own screen as well as in the
/// status, so a failure is never ambiguous about which session it belongs to.
fn report_end(status: &Arc<Mutex<Status>>, terminal: &Arc<Mutex<Terminal>>, message: String) {
    status.lock().unwrap_or_else(|e| e.into_inner()).error = Some(message.clone());
    terminal
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .note_error(&format!("[错误] {message}"));
}

pub struct Session {
    pub terminal: Arc<Mutex<Terminal>>,
    pub status: Arc<Mutex<Status>>,
    pub traffic: Arc<Traffic>,
    pub kind: SessionKind,
    pub pid: Option<u32>,
    pub size: (u16, u16),
    pub remote: Option<Arc<crate::remote::Connection>>,
    /// False once the session has actually been started.
    detached: bool,
    /// A pane that exists but has no connection yet. The status bar spins while
    /// any of these is on screen.
    pending: bool,
    input: SyncSender<Control>,
    stop: Arc<AtomicBool>,
    killer: Option<Box<dyn ChildKiller + Send + Sync>>,
    /// A serial device is exclusive, so its workers are joined on drop to
    /// guarantee the port is released before the next connect can open it.
    serial_workers: Vec<thread::JoinHandle<()>>,
    /// Keeps `auto` from selecting a device this process already has open.
    _serial_guard: Option<crate::serial::Guard>,
}

impl Session {
    /// How the tab strip should light this session up.
    pub fn link(&self) -> SessionStatus {
        if self.detached {
            return SessionStatus::Detached;
        }
        let status = self.status.lock().unwrap_or_else(|e| e.into_inner());
        match (status.exit_code, status.eof, &status.error) {
            // A clean exit is a normal end, not a failure.
            (Some(0), _, None) => SessionStatus::Detached,
            (Some(_), _, _) => SessionStatus::Lost,
            (None, true, _) | (None, _, Some(_)) => SessionStatus::Lost,
            (None, false, None) => SessionStatus::Live,
        }
    }

    pub fn disconnected(kind: SessionKind, scrollback: usize) -> Self {
        Self::disconnected_reusing(kind, scrollback, None)
    }

    /// A pane with no worker behind it. `previous` lets a failed connection keep
    /// the screen it was writing its own diagnostics into. The exit status is
    /// what makes the pane offer a retry.
    pub fn disconnected_reusing(
        kind: SessionKind,
        scrollback: usize,
        previous: Option<Arc<Mutex<Terminal>>>,
    ) -> Self {
        let (input, _) = mpsc::sync_channel(1);
        let terminal =
            previous.unwrap_or_else(|| Arc::new(Mutex::new(Terminal::new(30, 100, scrollback))));
        Self {
            terminal,
            status: Arc::new(Mutex::new(Status {
                exit_code: Some(0),
                eof: true,
                error: None,
            })),
            traffic: Arc::new(Traffic::default()),
            kind,
            pid: None,
            size: (30, 100),
            remote: None,
            detached: true,
            pending: false,
            input,
            stop: Arc::new(AtomicBool::new(true)),
            killer: None,
            serial_workers: Vec::new(),
            _serial_guard: None,
        }
    }

    /// A pane that exists before its connection does. It has no worker yet, but
    /// it does have a screen, which is where the connection's own notices and
    /// failures are written so they are never mistaken for another session's.
    pub fn connecting(kind: SessionKind, scrollback: usize) -> Self {
        let (input, _) = mpsc::sync_channel(1);
        Self {
            terminal: Arc::new(Mutex::new(Terminal::new(30, 100, scrollback))),
            status: Arc::new(Mutex::new(Status::default())),
            traffic: Arc::new(Traffic::default()),
            kind,
            pid: None,
            size: (30, 100),
            remote: None,
            detached: true,
            pending: true,
            input,
            stop: Arc::new(AtomicBool::new(true)),
            killer: None,
            serial_workers: Vec::new(),
            _serial_guard: None,
        }
    }

    /// Whether this pane is still waiting for its connection.
    pub fn pending(&self) -> bool {
        self.pending
    }

    /// Writes a notice into this session's own screen.
    pub fn note(&self, line: &str) {
        self.terminal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .note(line);
    }

    /// Writes a failure into this session's own screen.
    pub fn note_error(&self, line: &str) {
        self.terminal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .note_error(line);
    }

    /// Blank lines separating a new run of notices from older output.
    pub fn separator(&self) {
        self.terminal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .separator();
    }

    /// The screen a new session writes into: the previous one when reconnecting,
    /// so its scrollback survives the drop, otherwise a fresh bounded screen.
    fn screen(
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
                            terminal.process(&bytes[..n]);
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
        let pair = native_pty_system()
            .openpty(size)
            .context("无法创建系统 PTY（Windows 需要 10 1809 或更新版本）")?;
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
                            terminal.process(&bytes[..n]);
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
                                    let replies = { let mut t=out.lock().unwrap(); t.process(&data); std::mem::take(&mut t.parser.callbacks_mut().replies) };
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
                        // Closed without an exit status: the transport dropped
                        // rather than the shell exiting on its own.
                        state.lock().unwrap_or_else(|e| e.into_inner()).exit_code = Some(255);
                        report_end(&state, &out, "连接已断开（未收到退出状态）".into());
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

    /// Take over the session channel for a terminal-initiated ZMODEM transfer
    /// (`rz`/`sz` typed at the remote prompt). Ends when the transfer task
    /// drops the returned stream.
    pub fn begin_terminal_zmodem(&self) -> Result<BridgeStream> {
        anyhow::ensure!(self.remote.is_some(), "仅内置 SSH 会话支持 ZMODEM 抓取");
        let (to_transfer, to_transfer_rx) = tokio::sync::mpsc::unbounded_channel();
        let (from_transfer_tx, from_transfer) = tokio::sync::mpsc::unbounded_channel();
        self.input
            .try_send(Control::ZmodemBridge {
                to_transfer,
                from_transfer,
            })
            .context("会话已关闭，无法开始 ZMODEM 传输")?;
        Ok(BridgeStream {
            rx: to_transfer_rx,
            tx: from_transfer_tx,
            buffer: Vec::new(),
            position: 0,
        })
    }

    pub fn write(&self, bytes: Vec<u8>) -> Result<()> {
        if bytes.len() > 1024 * 1024 {
            bail!("单次粘贴不能超过 1 MiB");
        }
        {
            let status = self.status.lock().unwrap();
            if status.exit_code.is_some() {
                bail!("会话已退出，请重新启动");
            }
            // A session can be finished without ever producing an exit code — an
            // SSH transport that dropped, for instance. Without this check the
            // write reaches the channel, which is already closed, and the user is
            // shown the channel's own error, which means nothing to them.
            if status.eof || status.error.is_some() {
                bail!("会话已断开，请重新连接");
            }
        }
        self.input.try_send(Control::Write(bytes)).map_err(|error| {
            match error {
                // The reader went away between the check above and here.
                std::sync::mpsc::TrySendError::Disconnected(_) => {
                    anyhow::anyhow!("会话已断开，请重新连接")
                }
                std::sync::mpsc::TrySendError::Full(_) => {
                    anyhow::anyhow!("终端输入过快，请稍后重试")
                }
            }
        })
    }

    /// Follows the pane size. This is best effort: once a session has ended its
    /// input channel is closed, and that is already told by the status line, so
    /// a dead session must not turn every later resize into a spurious error.
    /// The size is recorded even when the send fails, otherwise the mismatch
    /// would be retried — and reported — on every frame.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let size = (rows.max(1), cols.max(1));
        if self.size != size {
            let _ = self.input.try_send(Control::Resize(PtySize {
                rows: size.0,
                cols: size.1,
                pixel_width: 0,
                pixel_height: 0,
            }));
            self.size = size;
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(killer) = &mut self.killer {
            let _ = killer.kill();
        }
        // The control worker closes ConPTY while the reader keeps draining it;
        // ClosePseudoConsole must never block the UI thread. Serial sessions
        // are the exception: the device is exclusive, so the workers are joined
        // to close the port before the same one can be reopened. The 50 ms read
        // timeout bounds the wait.
        for worker in self.serial_workers.drain(..) {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// A pane that exists before its connection has to be able to carry the
    /// connection's own notices, and it must not be offered as finished.
    #[test]
    fn a_connecting_pane_has_a_console_and_no_retry_offer() {
        let session = Session::connecting(SessionKind::Local("cmd".into()), 100);
        assert_eq!(session.link(), SessionStatus::Detached);
        assert!(session.status.lock().unwrap().exit_code.is_none());
        session.note("connect to COM3");
        session.note_error("connect failed: busy");
        let text = session.terminal.lock().unwrap().parser.screen().contents();
        assert!(text.contains("connect to COM3"), "{text}");
        assert!(text.contains("connect failed: busy"), "{text}");
    }

    /// A finished session has no worker left to receive a resize, and that must
    /// stay invisible: the status line is what reports the disconnect. The size
    /// still has to be recorded, or the same failed send would be repeated on
    /// every frame and the raw channel error would surface in the UI.
    #[test]
    fn resize_after_the_session_ends_is_silent_and_settles() {
        let mut session = Session::disconnected(SessionKind::Local("cmd".into()), 100);
        session.resize(40, 120);
        assert_eq!(session.size, (40, 120));
        session.resize(40, 120);
        assert_eq!(session.size, (40, 120));
    }

    #[test]
    fn a_missing_serial_port_fails_to_spawn_without_panicking() {
        let result = Session::spawn(
            SessionKind::Serial(SerialProfile {
                port: "COM199".into(),
                ..Default::default()
            }),
            100,
            Arc::new(|| {}),
        );
        let error = result.err().expect("COM199 must not exist");
        assert!(format!("{error:#}").contains("COM199"), "{error:#}");
    }

    #[test]
    fn remote_options_cannot_be_injected_through_destination() {
        for host in [
            "-oProxyCommand=calc",
            "host name",
            "a\ncommand",
            "user@host",
        ] {
            let p = RemoteProfile {
                host: host.into(),
                ..Default::default()
            };
            assert!(SessionKind::Ssh(p).command().is_err());
        }
        let p = RemoteProfile {
            host: "::1".into(),
            user: "test".into(),
            ..Default::default()
        };
        assert!(SessionKind::Ssh(p).command().is_ok());
    }
}
