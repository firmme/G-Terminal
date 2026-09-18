//! The updater UI: the window that reports a check, download and restart.

use super::*;

impl App {
    /// Kicks off a background check and leaves the result in `update_status`.
    pub(super) fn check_updates(&mut self, ctx: &egui::Context) {
        self.update_open = true;
        let status = self.update_status.clone();
        *status.lock().unwrap() = update::Status::Checking;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let next = update::check().unwrap_or_else(update::Status::Failed);
            *status.lock().unwrap() = next;
            ctx.request_repaint();
        });
    }

    /// Downloads the release and lets `update::install` swap the bundle once
    /// this process is gone, then quits.
    pub(super) fn start_update(&mut self, ctx: &egui::Context, release: update::Release) {
        let (Some(url), Some(name)) = (release.asset, release.asset_name) else {
            return;
        };
        let status = self.update_status.clone();
        *status.lock().unwrap() = update::Status::Downloading;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result =
                update::download(&url, &name).and_then(|archive| update::install(&archive));
            match result {
                Ok(()) => {
                    *status.lock().unwrap() = update::Status::Ready;
                    ctx.request_repaint();
                    // Let the "正在重启" note paint before the process goes away.
                    std::thread::sleep(std::time::Duration::from_millis(600));
                    std::process::exit(0);
                }
                Err(error) => {
                    *status.lock().unwrap() = update::Status::Failed(error);
                    ctx.request_repaint();
                }
            }
        });
    }

    /// The 检查更新 window.
    pub(super) fn update_window(&mut self, ctx: &egui::Context, p: Palette) {
        if !self.update_open {
            return;
        }
        let status = self.update_status.lock().unwrap().clone();
        let mut open = true;
        let mut retry = false;
        let mut start = None;
        egui::Window::new("检查更新")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(440.0)
            .show(ctx, |ui| match &status {
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
                    if ui.button("重试").clicked() {
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
                    ui.horizontal(|ui| {
                        if release.asset.is_some() {
                            if ui.button("下载并更新").clicked() {
                                start = Some(release.clone());
                            }
                        } else {
                            ui.label(hint("当前平台没有预编译包，请打开发布页手动下载。", p));
                        }
                        if ui.button("打开发布页").clicked() {
                            let _ = crate::remote_ui::open_url(&release.url);
                        }
                    });
                }
                update::Status::Downloading => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("正在下载新版本…");
                    });
                }
                update::Status::Ready => {
                    ui.label("更新已就绪，正在重启…");
                }
            });
        self.update_open = open;
        if retry {
            self.check_updates(ctx);
        }
        if let Some(release) = start {
            self.start_update(ctx, release);
        }
    }
}
