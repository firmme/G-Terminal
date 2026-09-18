//! The serial-port owner prompt: which program is holding the port, and the two
//! ways out of that — close the program, or restart the device so its handles
//! stop working. Scanning runs on a worker because the handle walk is not
//! instant, and both actions are the user's to confirm.

use super::*;

/// One prompt's worth of state, shared with whatever worker is running.
pub(super) struct PortOwnerWindow {
    port: String,
    state: Arc<Mutex<OwnerState>>,
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
    pub(super) fn new(port: String, ctx: &egui::Context) -> Self {
        let window = Self {
            port,
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

    /// Draws the window, returning whether it should stay open.
    pub(super) fn show(&mut self, ctx: &egui::Context, p: Palette) -> bool {
        let mut open = true;
        let mut rescan = false;
        let mut kill = None;
        let mut release = false;
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
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("端口");
                    editing::field_with(ui, &mut self.port, |edit| {
                        edit.desired_width(200.0)
                            .hint_text(format!("{} 或 auto", serial::PORT_EXAMPLE))
                    });
                    if ui.button("查找").clicked() {
                        rescan = true;
                    }
                });

                match &phase {
                    Phase::Scanning => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(hint("正在查找占用该端口的程序…", p));
                        });
                        // The spinner has to keep turning, and the worker's
                        // wake alone would only give it one frame.
                        ctx.request_repaint_after(std::time::Duration::from_millis(150));
                    }
                    Phase::Failed(error) => {
                        ui.colored_label(p.danger, error.as_str());
                    }
                    Phase::Listed(report) => {
                        if report.owners.is_empty() {
                            ui.label(hint("没有发现其它程序持有该端口。", p));
                        }
                        for owner in &report.owners {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&owner.name).strong().color(p.text));
                                ui.label(hint(&format!("PID {}", owner.pid), p));
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if owner.is_self() {
                                        ui.label(hint("本程序自己", p));
                                    } else if owner.killable {
                                        if ui
                                            .button("结束进程")
                                            .on_hover_text("先请求关闭窗口，再强制结束")
                                            .clicked()
                                        {
                                            kill = Some(owner.pid);
                                        }
                                    } else {
                                        ui.label(hint("需要管理员权限", p));
                                    }
                                });
                            });
                            if let Some(path) = &owner.path {
                                ui.label(hint(&path.display().to_string(), p));
                            }
                        }
                        if report.hidden > 0 {
                            ui.label(hint(
                                &format!(
                                    "另有 {} 个进程的句柄没有权限查看；以管理员身份运行本程序可见。",
                                    report.hidden
                                ),
                                p,
                            ));
                        }
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui
                                .button("重新查找")
                                .on_hover_text("关闭其它程序后再查一次")
                                .clicked()
                            {
                                rescan = true;
                            }
                            if g_terminal::port_owner::CAN_RELEASE
                                && ui
                                    .button("重启设备")
                                    .on_hover_text("禁用再启用该串口设备，让占用者的句柄失效")
                                    .clicked()
                            {
                                release = true;
                            }
                        });
                        ui.label(hint(
                            "Windows 的串口是独占的，两个程序无法同时打开；\
                             Linux 与 macOS 通常允许重复打开，但两边的数据会互相抢。",
                            p,
                        ));
                        if g_terminal::port_owner::CAN_RELEASE {
                            ui.label(hint(
                                "重启设备需要管理员权限；占用者可能随后重新打开该端口。",
                                p,
                            ));
                        }
                        if !g_terminal::port_owner::elevated() {
                            ui.label(hint(
                                "未以管理员身份运行：其它用户的进程可能不会列出，也无法结束。",
                                p,
                            ));
                        }
                    }
                }

                if let Some(note) = &note {
                    ui.separator();
                    ui.colored_label(p.text, note.as_str());
                }
            });

        if rescan {
            self.rescan(ctx);
        }
        if let Some(pid) = kill {
            self.act(ctx, move || g_terminal::port_owner::kill(pid));
        }
        if release {
            let port = self.port.clone();
            self.act(ctx, move || g_terminal::port_owner::release(&port));
        }
        open
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
                    assert!(window.show(ctx, palette), "the window must stay open");
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
