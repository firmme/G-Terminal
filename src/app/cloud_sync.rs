//! UI lifecycle for OneDrive. Workers own all network/keychain operations.
use super::*;
use g_terminal::sync::{self, Catalog, Preference};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

enum Completion {
    Login,
    Sync(sync::Outcome),
    Disconnect,
}

pub(super) struct CloudSync {
    receiver: Option<mpsc::Receiver<Result<Completion, String>>>,
    cancel: Arc<AtomicBool>,
    message: String,
    failed: bool,
    conflict: bool,
    next_check: Instant,
    observed: Option<Catalog>,
    dirty_at: Option<Instant>,
    failures: u32,
    inspect_at: Instant,
    login_pending: bool,
    focused: bool,
    background_round: u32,
    last_started: Option<Instant>,
}

impl Default for CloudSync {
    fn default() -> Self {
        Self {
            receiver: None,
            cancel: Arc::new(AtomicBool::new(false)),
            message: "尚未连接 OneDrive".into(),
            failed: false,
            conflict: false,
            next_check: Instant::now(),
            observed: None,
            dirty_at: None,
            failures: 0,
            inspect_at: Instant::now(),
            login_pending: false,
            focused: true,
            background_round: 0,
            last_started: None,
        }
    }
}

impl Drop for CloudSync {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl CloudSync {
    fn update_focus(&mut self, focused: bool, now: Instant) {
        if self.focused == focused {
            return;
        }
        self.focused = focused;
        self.background_round = 0;
        if focused {
            let earliest = self
                .last_started
                .map_or(now, |last| (last + Duration::from_secs(30)).max(now));
            self.next_check = if self.failed {
                self.next_check.max(earliest)
            } else {
                earliest
            };
        }
    }

    fn periodic_interval(&self) -> Duration {
        Duration::from_secs(if self.focused {
            120
        } else {
            (120u64 << self.background_round.min(4)).min(1800)
        })
    }

    fn auto_allowed(&self, now: Instant) -> bool {
        self.last_started
            .is_none_or(|last| now.saturating_duration_since(last) >= Duration::from_secs(30))
    }

    fn spawn(
        &mut self,
        ctx: &egui::Context,
        message: &str,
        task: impl FnOnce(Arc<AtomicBool>) -> anyhow::Result<Completion> + Send + 'static,
    ) {
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.message = message.into();
        self.failed = false;
        self.conflict = false;
        self.login_pending = false;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = task(cancel).map_err(|error| format!("{error:#}"));
            let _ = sender.send(result);
            ctx.request_repaint();
        });
    }
}

impl App {
    pub(super) fn preview_cloud_status(&mut self, failed: bool) {
        self.settings.onedrive_enabled = failed;
        self.cloud_sync.failed = failed;
        self.cloud_sync.message = if failed {
            "同步失败，请检查网络后重试。"
        } else {
            "尚未连接 OneDrive"
        }
        .into();
    }
    fn start_cloud_login(&mut self, ctx: &egui::Context) {
        if self.cloud_sync.receiver.is_some() || self.screenshot.is_some() {
            return;
        }
        self.cloud_sync.spawn(
            ctx,
            "请在浏览器登录微软账号并授权…",
            |cancel| {
                sync::login(
                    |url| remote_ui::open_url(url).map_err(anyhow::Error::msg),
                    &cancel,
                )?;
                Ok(Completion::Login)
            },
        );
        self.cloud_sync.login_pending = true;
    }

    /// A quiet status glyph; details and actions only appear on demand.
    pub(super) fn cloud_status_button(&mut self, ui: &mut egui::Ui, p: Palette) {
        let busy = self.cloud_sync.receiver.is_some();
        let enabled = self.settings.onedrive_enabled;
        let failed = self.cloud_sync.failed;
        let title = if failed {
            "连接同步失败"
        } else if busy && self.cloud_sync.login_pending {
            "正在登录 OneDrive"
        } else if busy {
            "正在同步连接"
        } else if !enabled {
            "未登录 OneDrive"
        } else if !self.settings.onedrive_auto_sync {
            "自动同步已关闭"
        } else if self.cloud_sync.observed.is_none() {
            "等待同步"
        } else {
            "OneDrive 连接同步"
        };
        let mut colors = p;
        if failed {
            colors.muted = p.danger;
            colors.text = p.danger;
        }
        let response = icons::icon_button(
            ui,
            if enabled {
                icons::Icon::Cloud
            } else {
                icons::Icon::CloudOff
            },
            colors,
            icons::Size::Row,
        )
        .on_hover_text(title);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, title));
        if busy {
            let phase = ui.input(|input| input.time) as f32;
            let opacity = 0.35 + 0.45 * (phase * 3.0).sin().abs();
            ui.painter().circle_filled(
                response.rect.right_bottom() - egui::vec2(4.0, 3.0),
                1.5,
                p.muted.gamma_multiply(opacity),
            );
            ui.ctx().request_repaint_after(Duration::from_millis(80));
        }
        let ctx = ui.ctx().clone();
        egui::Popup::menu(&response).show(|ui| {
            ui.set_width(264.0_f32.min(ctx.content_rect().width() - 32.0));
            ui.label(RichText::new(title).strong());
            if failed {
                ui.label(
                    self.cloud_sync
                        .message
                        .lines()
                        .next()
                        .unwrap_or("同步失败，请重试"),
                );
                egui::CollapsingHeader::new("错误详情").show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(140.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(&self.cloud_sync.message)
                                    .wrap()
                                    .selectable(true),
                            );
                        });
                    if ui.small_button("复制详情").clicked() {
                        ctx.copy_text(self.cloud_sync.message.clone());
                    }
                });
            } else if enabled {
                ui.label(hint(
                    if busy {
                        "正在更新连接列表"
                    } else {
                        &self.cloud_sync.message
                    },
                    p,
                ));
            } else {
                ui.label(hint("登录后可在多台设备之间同步连接列表。", p));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!busy && self.screenshot.is_none(), |ui| {
                    if enabled {
                        if ui
                            .button(if failed { "重试" } else { "立即同步" })
                            .clicked()
                        {
                            self.start_cloud_sync(&ctx, Preference::Ask);
                            ui.close();
                        }
                    } else if ui.button("登录 OneDrive").clicked() {
                        self.start_cloud_login(&ctx);
                        ui.close();
                    }
                });
                if ui.button("同步设置").clicked() {
                    self.settings_open = true;
                    ui.close();
                }
            });
        });
    }

    fn start_cloud_sync(&mut self, ctx: &egui::Context, preference: Preference) {
        if self.cloud_sync.receiver.is_some() || self.screenshot.is_some() {
            return;
        }
        sync::ensure_ids(&mut self.settings);
        let input = Catalog::from_settings(&self.settings);
        self.cloud_sync.last_started = Some(Instant::now());
        self.cloud_sync.dirty_at = None;
        self.cloud_sync.observed = Some(input.clone());
        self.cloud_sync.spawn(ctx, "正在同步连接列表…", move |_| {
            sync::synchronise(input, preference).map(Completion::Sync)
        });
    }

    pub(super) fn tick_cloud_sync(&mut self, ctx: &egui::Context) {
        if self.screenshot.is_some() || cfg!(test) {
            return;
        }
        let focused = ctx.input(|input| {
            let viewport = input.viewport();
            viewport.focused.unwrap_or(true) && !viewport.minimized.unwrap_or(false)
        });
        self.cloud_sync.update_focus(focused, Instant::now());
        // Keep an open connection/group editor's draft stable. Once it closes,
        // rebase any saved edits over the worker's result before applying it.
        if (self.remote_open || self.groups_open) && self.cloud_sync.receiver.is_some() {
            ctx.request_repaint_after(Duration::from_secs(1));
            return;
        }
        if let Some(result) = self
            .cloud_sync
            .receiver
            .as_ref()
            .and_then(|receiver| match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("同步工作线程异常退出，请重试".into()))
                }
                Err(mpsc::TryRecvError::Empty) => None,
            })
        {
            self.cloud_sync.receiver = None;
            let applied = match result {
                Ok(Completion::Login) => {
                    self.settings.onedrive_enabled = true;
                    self.cloud_sync.observed = None;
                    self.persist();
                    self.cloud_sync.message = "已登录，准备同步…".into();
                    self.cloud_sync.next_check = Instant::now();
                    self.start_cloud_sync(ctx, Preference::Ask);
                    Ok(())
                }
                Ok(Completion::Disconnect) => {
                    self.settings.onedrive_enabled = false;
                    self.persist();
                    self.cloud_sync.message = "已断开，本机和云端连接列表均保留".into();
                    Ok(())
                }
                Ok(Completion::Sync(outcome)) => self.apply_cloud_sync(outcome),
                Err(error) => Err(error),
            };
            if let Err(error) = applied {
                self.cloud_sync.conflict = error.contains("同步冲突：");
                self.cloud_sync.failed = true;
                self.cloud_sync.message = error;
                self.cloud_sync.failures = self.cloud_sync.failures.saturating_add(1);
                self.cloud_sync.next_check = Instant::now()
                    + Duration::from_secs(
                        (30u64 * 2u64.pow(self.cloud_sync.failures.min(5))).min(900),
                    )
                    .max(if self.cloud_sync.focused {
                        Duration::ZERO
                    } else {
                        self.cloud_sync.periodic_interval()
                    });
                if !self.cloud_sync.focused {
                    self.cloud_sync.background_round =
                        self.cloud_sync.background_round.saturating_add(1);
                }
            }
        }
        if !self.settings.onedrive_enabled
            || !self.settings.onedrive_auto_sync
            || self.cloud_sync.receiver.is_some()
        {
            return;
        }
        // Serialising every connection on every terminal frame would make a
        // large saved list affect typing/render latency. Inspect at most once/s.
        if Instant::now() < self.cloud_sync.inspect_at {
            ctx.request_repaint_after(
                self.cloud_sync
                    .inspect_at
                    .saturating_duration_since(Instant::now()),
            );
            return;
        }
        self.cloud_sync.inspect_at = Instant::now() + Duration::from_secs(1);
        sync::ensure_ids(&mut self.settings);
        let current = Catalog::from_settings(&self.settings);
        if self.cloud_sync.observed.as_ref() != Some(&current) {
            self.cloud_sync.observed = Some(current);
            self.cloud_sync.dirty_at = Some(Instant::now());
        }
        let initial =
            self.cloud_sync.observed.is_some() && self.cloud_sync.next_check <= Instant::now();
        let changed = self
            .cloud_sync
            .dirty_at
            .is_some_and(|at| at.elapsed() >= Duration::from_secs(2));
        if self.settings.onedrive_auto_sync
            && !self.cloud_sync.conflict
            && ((changed && !self.cloud_sync.failed) || initial)
            && self.cloud_sync.auto_allowed(Instant::now())
            && sync::client_id().is_some()
        {
            self.start_cloud_sync(ctx, Preference::Ask);
        }
        ctx.request_repaint_after(Duration::from_secs(1));
    }

    fn apply_cloud_sync(&mut self, outcome: sync::Outcome) -> Result<(), String> {
        sync::ensure_ids(&mut self.settings);
        // Changes made while the network worker ran stay local and are uploaded
        // on the next pass; unrelated remote changes are still applied now.
        let current = Catalog::from_settings(&self.settings);
        let rebased = Catalog::merge(&outcome.input, &current, &outcome.merged, Preference::Local)
            .map_err(|error| error.to_string())?;
        let ssh_edit = self
            .editing_profile
            .and_then(|i| self.settings.profiles.get(i))
            .map(|p| p.sync_id.clone());
        let serial_edit = self
            .editing_serial
            .and_then(|i| self.settings.serial_profiles.get(i))
            .map(|p| p.sync_id.clone());
        #[cfg(not(test))]
        let before = self.settings.clone();
        rebased
            .apply(&mut self.settings)
            .map_err(|error| error.to_string())?;
        #[cfg(not(test))]
        self.settings.save().map_err(|error| {
            self.settings = before;
            error.to_string()
        })?;
        if let Some(id) = ssh_edit {
            self.editing_profile = self.settings.profiles.iter().position(|p| p.sync_id == id);
            if self.editing_profile.is_none() {
                self.remote_open = false;
            }
        }
        if let Some(id) = serial_edit {
            self.editing_serial = self
                .settings
                .serial_profiles
                .iter()
                .position(|p| p.sync_id == id);
            if self.editing_serial.is_none() {
                self.remote_open = false;
            }
        }
        outcome.commit().map_err(|error| error.to_string())?;
        self.cloud_sync.message = "连接列表已同步".into();
        self.cloud_sync.failed = false;
        self.cloud_sync.conflict = false;
        self.cloud_sync.failures = 0;
        self.cloud_sync.observed = Some(rebased.clone());
        self.cloud_sync.dirty_at = (rebased != outcome.merged).then(Instant::now);
        self.cloud_sync.next_check = Instant::now() + self.cloud_sync.periodic_interval();
        if !self.cloud_sync.focused {
            self.cloud_sync.background_round = self.cloud_sync.background_round.saturating_add(1);
        }
        Ok(())
    }

    pub(super) fn cloud_sync_settings(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        p: Palette,
    ) {
        crate::dialog::group_title(ui, "连接列表同步 · OneDrive", p);
        if sync::client_id().is_none() {
            ui.label(hint("OneDrive 尚未配置", p));
            return;
        }
        let busy = self.cloud_sync.receiver.is_some();
        ui.horizontal(|ui| {
            if busy {
                ui.spinner();
            }
            if self.cloud_sync.failed {
                ui.colored_label(p.danger, "同步失败")
                    .on_hover_text(&self.cloud_sync.message);
            } else {
                ui.label(&self.cloud_sync.message);
            }
        });
        let mut login = false;
        let mut disconnect = false;
        let mut sync_now = false;
        let mut preference = Preference::Ask;
        ui.add_enabled_ui(!busy && self.screenshot.is_none(), |ui| {
            ui.horizontal(|ui| {
                login = ui
                    .button(if self.settings.onedrive_enabled {
                        "切换账号"
                    } else {
                        "登录微软账号"
                    })
                    .clicked();
                if self.settings.onedrive_enabled {
                    sync_now = ui.button("立即同步").clicked();
                    disconnect = ui.button("断开同步").clicked();
                }
            });
            if self.settings.onedrive_enabled {
                if ui
                    .checkbox(&mut self.settings.onedrive_auto_sync, "自动同步")
                    .on_hover_text("修改后上传；前台每 2 分钟检查，后台逐步延长至 30 分钟；自动请求至少间隔 30 秒")
                    .changed()
                {
                    self.persist();
                }
                if self.cloud_sync.conflict {
                    ui.horizontal(|ui| {
                        if ui.button("冲突项使用本机").clicked() {
                            preference = Preference::Local;
                            sync_now = true;
                        }
                        if ui.button("冲突项使用云端").clicked() {
                            preference = Preference::Cloud;
                            sync_now = true;
                        }
                    });
                }
            }
        });
        if busy
            && self.cloud_sync.login_pending
            && ui
                .button("取消等待登录")
                .on_hover_text("浏览器登录最长等待 3 分钟；网络请求将在超时后结束")
                .clicked()
        {
            self.cloud_sync.cancel.store(true, Ordering::Relaxed);
        }
        if login {
            self.start_cloud_login(ctx);
        } else if disconnect {
            self.cloud_sync.spawn(ctx, "正在断开同步…", |_| {
                sync::disconnect()?;
                Ok(Completion::Disconnect)
            });
        } else if sync_now {
            self.start_cloud_sync(ctx, preference);
        }
    }
}

#[cfg(test)]
mod scheduling_tests {
    use super::*;

    #[test]
    fn background_backoff_caps_and_foreground_respects_minimum_interval() {
        let now = Instant::now();
        let mut state = CloudSync::default();
        state.last_started = Some(now);
        state.update_focus(false, now);
        for (round, seconds) in [
            (0, 120),
            (1, 240),
            (2, 480),
            (3, 960),
            (4, 1800),
            (50, 1800),
        ] {
            state.background_round = round;
            assert_eq!(state.periodic_interval(), Duration::from_secs(seconds));
        }
        state.update_focus(true, now + Duration::from_secs(5));
        assert_eq!(state.periodic_interval(), Duration::from_secs(120));
        assert_eq!(state.next_check, now + Duration::from_secs(30));
        assert!(!state.auto_allowed(now + Duration::from_secs(29)));
        assert!(state.auto_allowed(now + Duration::from_secs(30)));
        state.update_focus(false, now + Duration::from_secs(31));
        state.failed = true;
        state.next_check = now + Duration::from_secs(900);
        state.update_focus(true, now + Duration::from_secs(32));
        assert_eq!(state.next_check, now + Duration::from_secs(900));
    }
}
