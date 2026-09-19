//! The serial-port owner prompt: which program is holding the port, and the two
//! ways out of that — close the program, or restart the device so its handles
//! stop working. Scanning runs on a worker because the handle walk is not
//! instant, and both actions are the user's to confirm.

use super::*;

/// One prompt's worth of state, shared with whatever worker is running.
pub(super) struct PortOwnerWindow {
    port: String,
    /// Set when the window was opened by a connect that was refused or failed,
    /// so it can offer to reconnect once the port is free. A window opened from
    /// the menu has no failed connect to retry.
    retry: bool,
    state: Arc<Mutex<OwnerState>>,
}

/// What the window asks the app to do after a frame.
pub(super) struct PortOwnerOutcome {
    pub keep_open: bool,
    /// The port is free and the refused connect should be tried again.
    pub reconnect: bool,
}

struct OwnerState {
    phase: Phase,
    /// The last action's outcome, or the reason there is nothing to act on.
    note: Option<String>,
    /// Whether the note reports something that worked: a failure is shown in
    /// red, because a kill that did not happen must not read like one that did.
    note_ok: bool,
    /// PIDs that were asked to stop and did not, so their rows keep offering
    /// the privileged attempt even when the direct one was allowed.
    failed: Vec<u32>,
}

/// A serial connect waiting on the owner check. Windows refuses a second open
/// of a held port and Unix lets it through and splits the stream, so the check
/// runs before connecting — off the UI thread, because it opens the device and
/// may have to walk every handle in the system.
pub(super) struct SerialProbe {
    tab: u64,
    pane: u64,
    kind: SessionKind,
    previous: Option<Arc<Mutex<Terminal>>>,
    result: Arc<Mutex<Option<bool>>>,
}

impl SerialProbe {
    pub(super) fn start(
        tab: u64,
        pane: u64,
        kind: SessionKind,
        previous: Option<Arc<Mutex<Terminal>>>,
        ctx: &egui::Context,
    ) -> Self {
        let port = match &kind {
            SessionKind::Serial(profile) => profile.port.clone(),
            _ => String::new(),
        };
        let result = Arc::new(Mutex::new(None));
        let slot = result.clone();
        let wake = remote_ui::wake(ctx);
        std::thread::spawn(move || {
            // A check that failed to run must not block the connect; the open
            // itself then reports whatever is actually wrong.
            let free = g_terminal::port_owner::port_is_free(&port).unwrap_or(true);
            if let Ok(mut guard) = slot.lock() {
                *guard = Some(free);
            }
            wake();
        });
        Self {
            tab,
            pane,
            kind,
            previous,
            result,
        }
    }

    /// The answer, once the worker has one.
    pub(super) fn take(&self) -> Option<bool> {
        self.result.lock().ok().and_then(|guard| *guard)
    }

    pub(super) fn parts(self) -> (u64, u64, SessionKind, Option<Arc<Mutex<Terminal>>>) {
        (self.tab, self.pane, self.kind, self.previous)
    }
}

#[derive(Clone)]
enum Phase {
    Scanning,
    Listed(g_terminal::port_owner::Report),
    Failed(String),
}

/// An action that needs rights this process does not have, so it is run by an
/// elevated copy of the app.
enum Privileged {
    Kill(u32),
    Release(String),
}

/// Runs an elevated action and returns its message and whether it worked. The
/// user may take a moment to answer the authorization prompt, so the report is
/// waited for.
fn run_privileged(action: &Privileged) -> (String, bool) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let report = std::env::temp_dir().join(format!(
        "g-terminal-action-{}-{stamp}.txt",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&report);
    let started = match action {
        Privileged::Kill(pid) => g_terminal::port_owner::elevate_kill(*pid, &report),
        Privileged::Release(port) => g_terminal::port_owner::elevate_release(port, &report),
    };
    match started {
        // Refused before anything ran: the authorization was declined.
        Err(error) => (error, false),
        Ok(()) => match wait_for_report(&report) {
            // The helper writes "失败：…" when the action itself did not work.
            Some(message) => {
                let ok = !message.starts_with("失败");
                (message, ok)
            }
            None => ("已授权，但没有收到执行结果".into(), false),
        },
    }
}

/// Re-reads the port and reports which of `attempted` are still holding it.
fn confirm(port: &str, attempted: &[u32]) -> (Option<g_terminal::port_owner::Report>, Vec<u32>) {
    let report = g_terminal::port_owner::scan(port).ok();
    let still = match &report {
        Some(report) => attempted
            .iter()
            .copied()
            .filter(|pid| report.owners.iter().any(|owner| owner.pid == *pid))
            .collect(),
        None => Vec::new(),
    };
    (report, still)
}

/// Adds the "still holding the port" line to an action's message.
fn join_note(message: String, still: &[u32]) -> String {
    if still.is_empty() {
        return message;
    }
    let pids = still
        .iter()
        .map(|pid| pid.to_string())
        .collect::<Vec<_>>()
        .join("、");
    format!("{message}；PID {pids} 仍在占用该端口，可尝试「以管理员身份结束」或「重启设备」")
}

/// Waits for the elevated copy's report to appear. The copy is a separate
/// process the caller cannot read output from, and the user may take a moment
/// to answer the authorization prompt.
fn wait_for_report(path: &std::path::Path) -> Option<String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(path) {
            let _ = std::fs::remove_file(path);
            return Some(text);
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    let _ = std::fs::remove_file(path);
    None
}

impl PortOwnerWindow {
    pub(super) fn new(port: String, retry: bool, ctx: &egui::Context) -> Self {
        let window = Self {
            port,
            retry,
            state: Arc::new(Mutex::new(OwnerState {
                phase: Phase::Scanning,
                note: None,
                note_ok: true,
                failed: Vec::new(),
            })),
        };
        window.rescan(ctx);
        window
    }

    /// Starts a fresh scan, replacing whatever the last one found.
    fn rescan(&self, ctx: &egui::Context) {
        let state = self.state.clone();
        let port = self.port.clone();
        let wake = remote_ui::wake(ctx);
        if let Ok(mut guard) = state.lock() {
            guard.phase = Phase::Scanning;
            guard.note = None;
        }
        std::thread::spawn(move || {
            let phase = match g_terminal::port_owner::scan(&port) {
                Ok(report) => Phase::Listed(report),
                Err(error) => Phase::Failed(error),
            };
            if let Ok(mut guard) = state.lock() {
                guard.phase = phase;
            }
            wake();
        });
    }

    /// Runs one action on a worker, leaving its message in the note.
    fn act(
        &self,
        ctx: &egui::Context,
        action: impl FnOnce() -> Result<String, String> + Send + 'static,
    ) {
        let state = self.state.clone();
        let port = self.port.clone();
        let wake = remote_ui::wake(ctx);
        if let Ok(mut guard) = state.lock() {
            guard.note = Some("正在处理…".into());
            guard.note_ok = true;
        }
        std::thread::spawn(move || {
            let (message, ok) = match action() {
                Ok(message) => (message, true),
                Err(error) => (error, false),
            };
            let fresh = g_terminal::port_owner::scan(&port).ok();
            if let Ok(mut guard) = state.lock() {
                guard.note = Some(message);
                guard.note_ok = ok;
                // A kill or a restart changes who holds the port, so the list
                // is refreshed rather than left stale.
                if let Some(report) = fresh {
                    guard.phase = Phase::Listed(report);
                }
            }
            wake();
        });
    }

    /// Runs one action through an elevated copy of the app. The copy writes its
    /// outcome to a report file: a release build has no console, and the
    /// privileged process is not the one drawing this window.
    fn privileged(&self, ctx: &egui::Context, action: Privileged) {
        let state = self.state.clone();
        let port = self.port.clone();
        let wake = remote_ui::wake(ctx);
        if let Ok(mut guard) = state.lock() {
            guard.note = Some("等待管理员授权…".into());
            guard.note_ok = true;
        }
        std::thread::spawn(move || {
            let attempted = match &action {
                Privileged::Kill(pid) => vec![*pid],
                Privileged::Release(_) => Vec::new(),
            };
            let (message, ok) = run_privileged(&action);
            let (report, still) = confirm(&port, &attempted);
            if let Ok(mut guard) = state.lock() {
                guard.note = Some(join_note(message, &still));
                guard.note_ok = ok && still.is_empty();
                guard.failed = still;
                if let Some(report) = report {
                    guard.phase = Phase::Listed(report);
                }
            }
            wake();
        });
    }

    /// Ends the processes on a worker. One this user may not touch is retried
    /// through the elevated copy, so the button does not have to guess which
    /// kind it is.
    fn kill_owners(&self, ctx: &egui::Context, pids: Vec<u32>) {
        let state = self.state.clone();
        let port = self.port.clone();
        let wake = remote_ui::wake(ctx);
        if let Ok(mut guard) = state.lock() {
            guard.note = Some("正在结束…".into());
            guard.note_ok = true;
        }
        std::thread::spawn(move || {
            let mut lines = Vec::new();
            let mut failed = Vec::new();
            for pid in &pids {
                match g_terminal::port_owner::kill(*pid) {
                    Ok(message) => lines.push(message),
                    Err(error) if g_terminal::port_owner::CAN_ELEVATE => {
                        if let Ok(mut guard) = state.lock() {
                            guard.note = Some(format!("{error}；请在弹出的授权窗口中确认"));
                        }
                        let (message, ok) = run_privileged(&Privileged::Kill(*pid));
                        lines.push(message);
                        if !ok {
                            failed.push(*pid);
                        }
                    }
                    Err(error) => {
                        lines.push(error);
                        failed.push(*pid);
                    }
                }
            }
            // Whatever the calls said, a process still listed afterwards did not
            // go away — that is the answer the user needs, not a success message.
            let (report, still) = confirm(&port, &pids);
            failed.extend(still.iter().copied());
            failed.sort_unstable();
            failed.dedup();
            if let Ok(mut guard) = state.lock() {
                guard.note = Some(join_note(lines.join("；"), &still));
                guard.note_ok = failed.is_empty();
                guard.failed = failed;
                if let Some(report) = report {
                    guard.phase = Phase::Listed(report);
                }
            }
            wake();
        });
    }

    /// Draws the window and returns what it wants next.
    pub(super) fn show(&mut self, ctx: &egui::Context, p: Palette) -> PortOwnerOutcome {
        let mut open = true;
        let mut rescan = false;
        let mut kill: Option<Vec<u32>> = None;
        let mut release = false;
        let mut reconnect = false;
        let mut privileged: Option<Privileged> = None;
        // The state is copied out before drawing: the window's closure would
        // otherwise hold the lock while a button asks for another scan.
        let (phase, note, note_ok, failed) = match self.state.lock() {
            Ok(guard) => (
                guard.phase.clone(),
                guard.note.clone(),
                guard.note_ok,
                guard.failed.clone(),
            ),
            Err(_) => (Phase::Scanning, None, true, Vec::new()),
        };

        egui::Window::new("串口占用排查")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(400.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("端口");
                    editing::field_with(ui, &mut self.port, |edit| {
                        edit.desired_width(170.0).hint_text("端口或 auto")
                    });
                    if ui.button("查找").clicked() {
                        rescan = true;
                    }
                });
                ui.separator();

                match &phase {
                    Phase::Scanning => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(hint("正在查找占用程序…", p));
                        });
                        // The spinner has to keep turning, and the worker's
                        // wake alone would only give it one frame.
                        ctx.request_repaint_after(std::time::Duration::from_millis(150));
                    }
                    Phase::Failed(error) => {
                        ui.colored_label(p.danger, error.as_str());
                        if ui.button("重试").clicked() {
                            rescan = true;
                        }
                    }
                    Phase::Listed(report) => {
                        // This process is not a blocker: it is the one asking.
                        let blockers: Vec<_> =
                            report.owners.iter().filter(|o| !o.is_self()).collect();
                        if blockers.is_empty() {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("✓ 端口空闲").color(p.ok).strong());
                                if self.retry && ui.button("重新连接").clicked() {
                                    reconnect = true;
                                }
                            });
                            // Only worth saying when nothing was found: a holder
                            // may be one of the processes that cannot be read.
                            if report.hidden > 0 {
                                ui.label(hint(
                                    &format!(
                                        "另有 {} 个进程受保护；以管理员身份运行可查看。",
                                        report.hidden
                                    ),
                                    p,
                                ));
                            }
                        } else {
                            ui.label(
                                RichText::new(format!("⚠ 端口被 {} 个程序占用", blockers.len()))
                                    .color(p.warn)
                                    .strong(),
                            );
                            for owner in &blockers {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(&owner.name).color(p.text));
                                    ui.label(hint(&format!("PID {}", owner.pid), p));
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        let stuck = failed.contains(&owner.pid);
                                        if stuck && g_terminal::port_owner::elevated() {
                                            // Already elevated: more rights will
                                            // not help, the device has to go.
                                            ui.label(hint("未退出，请用「重启设备」", p));
                                        } else if !stuck && owner.killable {
                                            if ui.button("结束").clicked() {
                                                kill = Some(vec![owner.pid]);
                                            }
                                        } else if g_terminal::port_owner::CAN_ELEVATE {
                                            // Out of reach for this user, or the
                                            // direct attempt already failed.
                                            if ui
                                                .button("以管理员身份结束")
                                                .on_hover_text("会弹出管理员授权（UAC）")
                                                .clicked()
                                            {
                                                privileged = Some(Privileged::Kill(owner.pid));
                                            }
                                        } else {
                                            ui.label(hint("需管理员", p));
                                        }
                                    });
                                });
                            }
                            ui.horizontal(|ui| {
                                if ui.button("重新查找").clicked() {
                                    rescan = true;
                                }
                                if blockers.iter().filter(|o| o.killable).count() > 1
                                    && ui.button("全部结束").clicked()
                                {
                                    kill = Some(
                                        blockers
                                            .iter()
                                            .filter(|o| o.killable)
                                            .map(|o| o.pid)
                                            .collect(),
                                    );
                                }
                                if g_terminal::port_owner::CAN_RELEASE
                                    && ui
                                        .button("重启设备")
                                        .on_hover_text("禁用再启用设备，让占用者的句柄失效")
                                        .clicked()
                                {
                                    if g_terminal::port_owner::elevated() {
                                        release = true;
                                    } else if g_terminal::port_owner::CAN_ELEVATE {
                                        privileged = Some(Privileged::Release(self.port.clone()));
                                    } else {
                                        release = true;
                                    }
                                }
                            });
                            ui.label(hint("结束后重新连接。", p));
                        }
                    }
                }

                if let Some(note) = &note {
                    ui.separator();
                    ui.colored_label(if note_ok { p.ok } else { p.danger }, note.as_str());
                }
            });

        if rescan {
            self.rescan(ctx);
        }
        if let Some(pids) = kill {
            self.kill_owners(ctx, pids);
        }
        if release {
            let port = self.port.clone();
            self.act(ctx, move || g_terminal::port_owner::release(&port));
        }
        if let Some(action) = privileged {
            self.privileged(ctx, action);
        }
        PortOwnerOutcome {
            keep_open: open,
            reconnect,
        }
    }
}

impl App {
    /// The port the owner prompt opens with: whatever the serial picker has
    /// selected, else the first port on the machine, else nothing to guess from.
    pub(super) fn port_owner_default(&self) -> String {
        if let Some(picker) = &self.serial_picker {
            return picker.port.clone();
        }
        self.serial_ports
            .first()
            .map(|port| port.name.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
impl PortOwnerWindow {
    /// A window whose scan has already finished, for the drawing tests.
    fn listed(port: &str, report: g_terminal::port_owner::Report) -> Self {
        Self::settled(port, Phase::Listed(report))
    }

    /// A window whose scan failed, for the drawing tests.
    fn failed(port: &str) -> Self {
        Self::settled(port, Phase::Failed("无法读取该串口".into()))
    }

    fn settled(port: &str, phase: Phase) -> Self {
        Self {
            port: port.into(),
            retry: true,
            state: Arc::new(Mutex::new(OwnerState {
                phase,
                note: Some("已结束进程（PID 4242）".into()),
                note_ok: true,
                failed: Vec::new(),
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use g_terminal::port_owner::{Owner, Report};
    use std::path::PathBuf;

    fn found() -> Report {
        Report {
            port: "COM3".into(),
            owners: vec![
                Owner {
                    pid: 4242,
                    name: "other-terminal.exe".into(),
                    path: Some(PathBuf::from("C:/tools/other-terminal.exe")),
                    killable: true,
                },
                Owner {
                    pid: 4243,
                    name: "as-admin.exe".into(),
                    path: None,
                    killable: false,
                },
                Owner {
                    pid: std::process::id(),
                    name: "g-terminal.exe".into(),
                    path: None,
                    killable: true,
                },
            ],
            hidden: 2,
        }
    }

    /// Every state has to survive a full layout pass: an empty result, owners
    /// with and without a path, one that needs admin, and this process itself.
    #[test]
    fn every_state_of_the_owner_window_draws() {
        let ctx = egui::Context::default();
        let palette = Palette::new(false);
        let mut listed = PortOwnerWindow::listed("COM3", found());
        let mut empty = PortOwnerWindow::listed(
            "COM1",
            Report {
                port: "COM1".into(),
                owners: Vec::new(),
                hidden: 0,
            },
        );
        let mut failed = PortOwnerWindow::failed("COM1");
        for window in [&mut listed, &mut empty, &mut failed] {
            for _ in 0..2 {
                let _ = ctx.run(Default::default(), |ctx| {
                    assert!(
                        window.show(ctx, palette).keep_open,
                        "the window must stay open"
                    );
                });
            }
        }
    }

    /// The default port follows the picker when it is open, so the button and
    /// the prompt agree about what is being looked at.
    #[test]
    fn the_default_port_follows_the_serial_picker() {
        let ctx = egui::Context::default();
        let mut app = App::new_context(&ctx, None);
        app.serial_ports = Vec::new();
        assert_eq!(app.port_owner_default(), "");
        app.serial_ports = vec![g_terminal::serial::SerialPortInfo {
            name: "COM7".into(),
            bluetooth: false,
        }];
        assert_eq!(app.port_owner_default(), "COM7");
        app.serial_picker = Some(SerialPicker {
            ports: Vec::new(),
            port: "COM9".into(),
            baud: 9600,
        });
        assert_eq!(app.port_owner_default(), "COM9");
    }
}
