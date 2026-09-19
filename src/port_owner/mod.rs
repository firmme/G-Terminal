//! Finding the program that is holding a serial port.
//!
//! Windows serial ports are exclusive, so a second opener fails outright; on
//! Unix the port usually opens anyway and the two readers split the stream.
//! Either way the useful answer is *which process* holds it, so the UI can let
//! the user close that program or restart the device instead of guessing. No
//! platform offers an API for the question, so each one walks its own list of
//! open handles here.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as imp;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as imp;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows as imp;

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
use unsupported as imp;

use std::path::PathBuf;

/// One process that has the port open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub pid: u32,
    /// The executable's file name, or a command name when that is all there is.
    pub name: String,
    /// The executable's path, when it could be read.
    pub path: Option<PathBuf>,
    /// The current user is allowed to end it (same user, or elevated).
    pub killable: bool,
}

impl Owner {
    /// Whether this is the running process itself; killing that would be suicide.
    pub fn is_self(&self) -> bool {
        self.pid == std::process::id()
    }
}

/// What one scan found.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// The port the scan actually looked at, after `auto` was resolved.
    pub port: String,
    pub owners: Vec<Owner>,
    /// Processes whose handles could not be read, because they belong to another
    /// user or a higher integrity level. A non-zero count means the list may be
    /// short rather than empty.
    pub hidden: usize,
}

/// Whether scanning a port's owners is implemented on this platform.
pub const SUPPORTED: bool = imp::SUPPORTED;

/// Whether restarting the device under the port is implemented.
pub const CAN_RELEASE: bool = imp::CAN_RELEASE;

/// Whether the app can ask for administrator rights by relaunching itself. A
/// task that is out of reach is not out of reach for its elevated copy, so the
/// prompt offers that instead of giving up.
pub const CAN_ELEVATE: bool = imp::CAN_ELEVATE;

/// Relaunches this app privileged to end `pid`, writing the outcome to
/// `report`. Returns once the elevation request was answered; the report
/// arrives later, when the privileged copy has run.
pub fn elevate_kill(pid: u32, report: &std::path::Path) -> Result<(), String> {
    imp::elevate_kill(pid, report)
}

/// Relaunches this app privileged to restart the device behind `port`.
pub fn elevate_release(port: &str, report: &std::path::Path) -> Result<(), String> {
    imp::elevate_release(port, report)
}

/// Whether the current process is elevated (administrator/root).
pub fn elevated() -> bool {
    imp::elevated()
}

/// Whether `port` can be opened right now. The check has to be cheap, because
/// it runs before every serial connect: on Windows a held port is refused at
/// once, so an open attempt answers it; on Unix a second open would succeed, so
/// the owner scan answers instead.
pub fn port_is_free(port: &str) -> Result<bool, String> {
    let resolved = crate::serial::resolve(port).map_err(|e| format!("{e:#}"))?;
    imp::port_is_free(&resolved)
}

/// Lists the processes holding `port`, resolving `auto` first.
pub fn scan(port: &str) -> Result<Report, String> {
    let resolved = crate::serial::resolve(port).map_err(|e| format!("{e:#}"))?;
    let mut report = imp::scan(&resolved)?;
    report.port = resolved;
    Ok(report)
}

/// Asks a process to stop, then forces it. Runs on a worker thread; the caller
/// waits for the returned message.
pub fn kill(pid: u32) -> Result<String, String> {
    imp::kill(pid)
}

/// Restarts the device behind `port`, which forces its holders' handles to fail
/// so the port can be reopened. Needs administrator rights on every platform
/// that implements it.
pub fn release(port: &str) -> Result<String, String> {
    let resolved = crate::serial::resolve(port).map_err(|e| format!("{e:#}"))?;
    imp::release(&resolved)
}
