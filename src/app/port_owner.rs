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
}

#[derive(Clone)]
enum Phase {
    Scanning,
    Listed(g_terminal::port_owner::Report),
    Failed(String),
}

impl PortOwnerWindow {
    pub(super) fn new(port: String, retry: bool, ctx: &egui::Context) -> Self {
        let window = Self {
            port,
            retry,
            state: Arc::new(Mutex::new(OwnerState {
                phase: Phase::Scanning,
                note: None,
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
        }
        std::thread::spawn(move || {
            let message = match action() {
                Ok(message) => message,
                Err(error) => error,
            };
            let fresh = g_terminal::port_owner::scan(&port).ok();
            if let Ok(mut guard) = state.lock() {
                guard.note = Some(message);
                // A kill or a restart changes who holds the port, so the list
                // is refreshed rather than left stale.
                if let Some(report) = fresh {
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
        // The state is copied out before drawing: the window's closure would
        // otherwise hold the lock while a button asks for another scan.
        let (phase, note) = match self.state.lock() {
            Ok(guard) => (guard.phase.clone(), guard.note.clone()),
            Err(_) => (Phase::Scanning, None),
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
                                        if owner.killable {
                                            if ui.button("结束").clicked() {
                                                kill = Some(vec![owner.pid]);
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
                                    release = true;
                                }
                            });
                            ui.label(hint("结束后重新连接。", p));
                        }
                    }
                }

                if let Some(note) = &note {
                    ui.separator();
                    ui.label(note.as_str());
                }
            });

        if rescan {
            self.rescan(ctx);
        }
        if let Some(pids) = kill {
            self.act(ctx, move || {
                let mut lines = Vec::new();
                for pid in pids {
                    lines.push(g_terminal::port_owner::kill(pid).unwrap_or_else(|e| e));
                }
                Ok(lines.join("；"))
            });
        }
        if release {
            let port = self.port.clone();
            self.act(ctx, move || g_terminal::port_owner::release(&port));
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
