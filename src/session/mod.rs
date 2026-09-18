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

mod bridge;
mod local;
mod remote;
mod shells;
mod status;
#[cfg(test)]
mod tests;
pub use bridge::BridgeStream;
use bridge::*;
pub use shells::{LocalShell, SessionKind, local_shells, shell_label};
use status::*;
pub use status::{SessionStatus, Status, Traffic};

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
