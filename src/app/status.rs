//! Status bar and traffic meters.

use super::*;

impl App {
    pub(super) fn tick_status(&mut self, ctx: &egui::Context) {
        let rates_animating = self.sample_rates();
        let connecting = self.connecting();
        if connecting {
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
        } else if rates_animating {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        if let Some((_, at)) = &self.toast {
            if at.elapsed() >= TOAST_LIFETIME {
                self.toast = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
    }

    pub(super) fn status_bar(
        &mut self,
        ctx: &egui::Context,
        action: &mut Option<Action>,
        p: Palette,
    ) {
        let connecting = self.connecting();
        let down_rate = self.rates.down_per_sec;
        let up_rate = self.rates.up_per_sec;
        let resizable = !cfg!(target_os = "macos")
            && ctx.input(|i| {
                !i.viewport().fullscreen.unwrap_or(false)
                    && !i.viewport().maximized.unwrap_or(false)
            });
        egui::TopBottomPanel::bottom("status")
            .frame(egui::Frame::new().fill(p.panel).inner_margin(egui::Margin {
                left: 12,
                right: if resizable { 24 } else { 12 },
                top: 5,
                bottom: 5,
            }))
            .show(ctx, |ui| {
                if let Some(error) = self.error.clone() {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(p.danger, error);
                        if ui.small_button("×").clicked() {
                            self.error = None;
                        }
                    });
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if let Some(t) = self.tabs.get(self.active) {
                        let s = &t.panes[t.focused].session;
                        let link = s.link();
                        let state = s.status.lock().unwrap();
                        let (state_color, state_text) = if s.pending() {
                            (p.warn, "连接中…".to_string())
                        } else {
                            match link {
                                SessionStatus::Live => (p.ok, "运行中".to_string()),
                                SessionStatus::Detached => (
                                    p.muted,
                                    state.exit_code.map_or_else(
                                        || "未连接 · 可重新连接".to_string(),
                                        |code| format!("已退出 {code} · 可重新连接"),
                                    ),
                                ),
                                SessionStatus::Lost => {
                                    (p.danger, "连接中断 · 可重新连接".to_string())
                                }
                            }
                        };
                        let (dot, _) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), Sense::hover());
                        ui.painter().circle_filled(dot.center(), 4.0, state_color);
                        ui.colored_label(state_color, state_text);
                        ui.separator();
                        ui.label(hint(
                            &format!(
                                "{}×{} · {} · {}/{}",
                                s.size.1,
                                s.size.0,
                                if s.remote.is_some() {
                                    "SSH"
                                } else if cfg!(windows) {
                                    "ConPTY"
                                } else {
                                    "PTY"
                                },
                                t.focused + 1,
                                t.panes.len()
                            ),
                            p,
                        ));
                        if let Some(e) = &state.error {
                            ui.colored_label(p.danger, e);
                        }
                        if let Some(c) = &s.remote {
                            let forwards = c.forwarding.lock().unwrap().clone();
                            ui.menu_button(format!("转发 {}", forwards.len()), |ui| {
                                for status in &forwards {
                                    ui.label(status);
                                }
                                if ui.button("停止全部转发").clicked() {
                                    c.stop_forwards();
                                    ui.close();
                                }
                            });
                            if icons::icon_label_button(ui, icons::Icon::File, "文件", p).clicked()
                            {
                                *action = Some(Action::Files);
                            }
                        }
                    } else {
                        ui.label(hint("就绪", p));
                    }
                    if let Some((text, at)) = &self.toast
                        && at.elapsed() < TOAST_LIFETIME
                    {
                        ui.colored_label(p.accent, text);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                        if icons::icon_button(
                            ui,
                            if fullscreen {
                                icons::Icon::ExitFullscreen
                            } else {
                                icons::Icon::Fullscreen
                            },
                            p,
                            icons::Size::Row,
                        )
                        .on_hover_text(if fullscreen {
                            "退出全屏"
                        } else {
                            "全屏显示"
                        })
                        .clicked()
                        {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
                        }
                        if connecting {
                            connecting_spinner(ui, p);
                        }
                        let rates = format!(
                            "上行 {}\n下行 {}",
                            format_bitrate(up_rate),
                            format_bitrate(down_rate)
                        );
                        ui.label(RichText::new("↓").color(if self.rates.down_active {
                            p.ok
                        } else {
                            p.muted
                        }))
                        .on_hover_text(&rates);
                        ui.label(RichText::new("↑").color(if self.rates.up_active {
                            p.danger
                        } else {
                            p.muted
                        }))
                        .on_hover_text(&rates);
                        if icons::icon_button(ui, icons::Icon::Search, p, icons::Size::Row)
                            .on_hover_text(format!("查找历史输出 ({})", accel("Ctrl+Shift+F")))
                            .clicked()
                        {
                            self.search_open = true;
                            self.search_focus = true;
                        }
                        if icons::icon_button(ui, icons::Icon::Info, p, icons::Size::Row)
                            .on_hover_text("当前终端快捷键帮助")
                            .clicked()
                        {
                            self.help_open = true;
                        }
                        if let Some(link) = self.active_link() {
                            let (icon, label, next) = if link == SessionStatus::Live {
                                (icons::Icon::Disconnect, "断开当前连接", Action::Disconnect)
                            } else {
                                (icons::Icon::Reconnect, "重新连接当前会话", Action::Restart)
                            };
                            if ui
                                .add_enabled_ui(!connecting, |ui| {
                                    icons::icon_button(ui, icon, p, icons::Size::Row)
                                        .on_hover_text(label)
                                })
                                .inner
                                .clicked()
                            {
                                *action = Some(next);
                            }
                        }
                        if icons::icon_button(ui, icons::Icon::Settings, p, icons::Size::Row)
                            .on_hover_text("偏好设置")
                            .clicked()
                        {
                            self.settings_open = true;
                        }
                        ui.label(hint("UTF-8", p));
                    });
                });
                if let Some(state) = self.files.as_ref().and_then(Files::operation_state)
                    && self.files.as_ref().is_some_and(Files::busy)
                {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("ZMODEM").color(p.accent));
                        ui.add(
                            egui::ProgressBar::new(state.fraction())
                                .desired_width(160.0)
                                .show_percentage(),
                        );
                        ui.label(hint(
                            &format!(
                                "{} · {} / {} · {}",
                                state.name,
                                state.message,
                                format_size(state.done),
                                format_size(state.total)
                            ),
                            p,
                        ));
                        if ui.small_button("打开传输窗口").clicked() {
                            self.files_open = true;
                        }
                        if ui.small_button("取消").clicked()
                            && let Some(files) = &self.files
                        {
                            files.cancel();
                        }
                    });
                    ctx.request_repaint_after(std::time::Duration::from_millis(150));
                }
            });
        if resizable {
            egui::Area::new(egui::Id::new("window-resize-grip"))
                .anchor(egui::Align2::RIGHT_BOTTOM, egui::Vec2::ZERO)
                .order(egui::Order::Middle)
                .movable(false)
                .fade_in(false)
                .show(ctx, |ui| {
                    let grip = resize_grip(ui, p);
                    if grip.hovered() || grip.dragged() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
                    }
                    if grip.drag_started()
                        || (grip.hovered() && ui.input(|i| i.pointer.primary_pressed()))
                    {
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::BeginResize(
                                egui::viewport::ResizeDirection::SouthEast,
                            ));
                    }
                });
        }
    }
}

pub(super) const TOAST_LIFETIME: std::time::Duration = std::time::Duration::from_millis(2500);

#[derive(Default)]
pub(super) struct RateMeter {
    pane: Option<u64>,
    at: Option<std::time::Instant>,
    up: u64,
    down: u64,
    last_up: u64,
    last_down: u64,
    up_at: Option<std::time::Instant>,
    down_at: Option<std::time::Instant>,
    pub(super) up_active: bool,
    pub(super) down_active: bool,
    pub(super) up_per_sec: f64,
    pub(super) down_per_sec: f64,
}

const RATE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
const TRAFFIC_ACTIVITY: std::time::Duration = std::time::Duration::from_millis(600);

impl RateMeter {
    pub(super) fn sample(&mut self, pane: u64, up: u64, down: u64) -> bool {
        self.sample_at(pane, up, down, std::time::Instant::now())
    }

    pub(super) fn sample_at(
        &mut self,
        pane: u64,
        up: u64,
        down: u64,
        now: std::time::Instant,
    ) -> bool {
        if self.pane != Some(pane) || up < self.last_up || down < self.last_down {
            *self = Self {
                pane: Some(pane),
                at: Some(now),
                up,
                down,
                last_up: up,
                last_down: down,
                ..Self::default()
            };
            return false;
        }
        if up > self.last_up {
            self.up_at = Some(now);
        }
        if down > self.last_down {
            self.down_at = Some(now);
        }
        self.last_up = up;
        self.last_down = down;
        self.up_active = self
            .up_at
            .is_some_and(|at| now.duration_since(at) < TRAFFIC_ACTIVITY);
        self.down_active = self
            .down_at
            .is_some_and(|at| now.duration_since(at) < TRAFFIC_ACTIVITY);
        if let Some(at) = self.at {
            let elapsed = now.duration_since(at);
            if elapsed >= RATE_INTERVAL {
                // Hold the displayed value for a whole second, independent of frame rate.
                self.up_per_sec = up.saturating_sub(self.up) as f64 / elapsed.as_secs_f64();
                self.down_per_sec = down.saturating_sub(self.down) as f64 / elapsed.as_secs_f64();
                self.at = Some(now);
                self.up = up;
                self.down = down;
            }
        }
        self.up_active
            || self.down_active
            || up != self.up
            || down != self.down
            || self.up_per_sec > 0.0
            || self.down_per_sec > 0.0
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
}

pub(super) fn format_bitrate(bytes_per_sec: f64) -> String {
    const UNITS: [&str; 4] = ["bps", "Kbps", "Mbps", "Gbps"];
    let mut value = (bytes_per_sec * 8.0).max(0.0);
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub(super) fn connecting_spinner(ui: &mut egui::Ui, p: Palette) {
    const RING: [(f32, f32); 8] = [
        (0.0, -1.0),
        (1.0, -1.0),
        (1.0, 0.0),
        (1.0, 1.0),
        (0.0, 1.0),
        (-1.0, 1.0),
        (-1.0, 0.0),
        (-1.0, -1.0),
    ];
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::hover());
    let tau = std::f32::consts::TAU;
    let head = (ui.input(|i| i.time) as f32 * tau * 0.8).rem_euclid(tau);
    let painter = ui.painter();
    for (index, (x, y)) in RING.iter().enumerate() {
        let angle = index as f32 / RING.len() as f32 * tau;
        let behind = (head - angle).rem_euclid(tau) / tau;
        let lit = (1.0 - behind).powi(2);
        painter.rect_filled(
            egui::Rect::from_center_size(
                rect.center() + egui::vec2(x * 4.5, y * 4.5),
                egui::vec2(3.0, 3.0),
            ),
            0,
            p.accent.gamma_multiply(0.12 + 0.88 * lit),
        );
    }
}

pub(super) fn resize_grip(ui: &mut egui::Ui, p: Palette) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::drag());
    let color = if response.hovered() || response.dragged() {
        p.accent
    } else {
        p.muted
    };
    for step in 0..3 {
        let offset = 3.0 + step as f32 * 4.5;
        ui.painter().line_segment(
            [
                egui::pos2(rect.right() - offset, rect.bottom() - 1.0),
                egui::pos2(rect.right() - 1.0, rect.bottom() - offset),
            ],
            egui::Stroke::new(1.0_f32, color),
        );
    }
    response
}
