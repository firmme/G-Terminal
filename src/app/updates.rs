//! The updater UI: the window that reports a check, download and restart.

use super::*;
use crate::icons::Icon;
use g_terminal::LockExt;

impl App {
    /// Kicks off a background check and leaves the result in `update_status`.
    pub(super) fn check_updates(&mut self, ctx: &egui::Context) {
        self.update_open = true;
        let status = self.update_status.clone();
        {
            let mut state = status.locked();
            if matches!(
                *state,
                update::Status::Checking | update::Status::Downloading | update::Status::Ready(_)
            ) {
                return;
            }
            *state = update::Status::Checking;
        }
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let next = update::check().unwrap_or_else(update::Status::Failed);
            *status.locked() = next;
            ctx.request_repaint();
        });
    }

    fn start_update(&mut self, ctx: &egui::Context, package: update::Package) {
        let status = self.update_status.clone();
        *status.locked() = update::Status::Downloading;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let next = match update::PreparedUpdate::prepare(&package) {
                Ok(prepared) => update::Status::Ready(prepared),
                Err(error) => update::Status::Failed(error),
            };
            *status.locked() = next;
            ctx.request_repaint();
        });
    }

    pub(super) fn launch_prepared_update(&self) -> Result<(), String> {
        match &mut *self.update_status.locked() {
            update::Status::Ready(prepared) => prepared.launch(),
            _ => Err("更新文件尚未准备完成".into()),
        }
    }

    /// The 检查更新 window.
    pub(super) fn update_window(&mut self, ctx: &egui::Context, p: Palette) {
        if !self.update_open {
            return;
        }
        let status = self.update_status.locked();
        let mut open = true;
        let mut retry = false;
        let mut start = None;
        let mut restart = false;
        crate::dialog::Dialog::new("检查更新", Icon::Refresh, p)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(400.0)
            .show(ctx, |ui| match &*status {
                update::Status::Idle => {}
                update::Status::Checking => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("正在检查 GitHub 上的最新版本…");
                    });
                }
                update::Status::UpToDate { current } => {
                    ui.label(format!("已是最新版本（{current}）。"));
                }
                update::Status::Failed(error) => {
                    ui.colored_label(p.danger, error);
                    if crate::dialog::primary(ui, "重试", p).clicked() {
                        retry = true;
                    }
                }
                update::Status::Available(release) => {
                    ui.label(format!(
                        "发现新版本 {}（当前 {}）。",
                        release.version,
                        update::current_version()
                    ));
                    if !release.notes.trim().is_empty() {
                        ui.separator();
                        egui::ScrollArea::vertical()
                            .max_height(160.0)
                            .show(ui, |ui| {
                                ui.add(egui::Label::new(hint(&release.notes, p)).wrap());
                            });
                    }
                    ui.separator();
                    crate::dialog::footer(ui, |ui| {
                        match &release.download {
                            update::Download::Available(package) => {
                                if crate::dialog::primary(ui, "下载更新", p).clicked() {
                                    start = Some(package.clone());
                                }
                            }
                            update::Download::MissingChecksum => {
                                ui.label(hint("发布缺少校验文件，请在发布页手动下载。", p));
                            }
                            update::Download::Unavailable => {
                                ui.label(hint("当前平台没有预编译包，请在发布页手动下载。", p));
                            }
                        }
                        if crate::dialog::secondary(ui, "打开发布页").clicked() {
                            let _ = crate::remote_ui::open_url(&release.url);
                        }
                    });
                }
                update::Status::Downloading => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("正在下载并校验更新…");
                    });
                }
                update::Status::Ready(_) => {
                    ui.label("更新已就绪。现在重启会断开当前连接。");
                    crate::dialog::footer(ui, |ui| {
                        restart = crate::dialog::primary(ui, "重启并安装", p).clicked();
                    });
                }
            });
        drop(status);
        self.update_open = open;
        if retry {
            self.check_updates(ctx);
        }
        if let Some(package) = start {
            self.start_update(ctx, package);
        }
        if restart {
            self.restart_for_update = true;
            self.request_exit(ctx);
            ctx.request_repaint();
        }
    }
}
