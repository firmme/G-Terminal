//! What a session reports: its link state, the console status line and the
//! traffic counters.

use super::*;

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
    /// The session's output rang the bell and nobody has looked yet. Read and
    /// cleared by the UI, so a background tab can show it.
    pub bell: std::sync::atomic::AtomicBool,
}

impl Traffic {
    pub(super) fn add_up(&self, bytes: usize) {
        self.up
            .fetch_add(bytes as u64, std::sync::atomic::Ordering::Relaxed);
    }
    pub(super) fn add_down(&self, bytes: usize) {
        self.down
            .fetch_add(bytes as u64, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Records why a session ended on the session's own screen as well as in the
/// status, so a failure is never ambiguous about which session it belongs to.
pub(super) fn report_end(
    status: &Arc<Mutex<Status>>,
    terminal: &Arc<Mutex<Terminal>>,
    message: String,
) {
    status.lock().unwrap_or_else(|e| e.into_inner()).error = Some(message.clone());
    terminal
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .note_error(&format!("[错误] {message}"));
}
