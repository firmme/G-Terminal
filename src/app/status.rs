//! Status bar and traffic meters.

use super::*;
use g_terminal::LockExt;

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
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::hover());
                        icons::draw(ui.painter(), rect, icons::Icon::Warning, p.danger, 1.3);
                        ui.colored_label(p.danger, error);
                        if icons::close_button(ui, p, "关闭").clicked() {
                            self.error = None;
                        }
                    });
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if let Some(t) = self.tabs.get(self.active) {
                        let s = &t.panes[t.focused].session;
                        let link = s.link();
                        let state = s.status.locked();
                        let (state_color, state_text) = if s.pending() {
                            (p.warn, "连接中".to_string())
                        } else {
                            match link {
                                SessionStatus::Live => (p.ok, "运行中".to_string()),
                                SessionStatus::Detached => (
                                    p.muted,
                                    state.exit_code.filter(|code| *code != 0).map_or_else(
                                        || "未连接".to_string(),
                                        |code| format!("已退出 {code}"),
                                    ),
                                ),
                                SessionStatus::Lost => (p.danger, "已断开".to_string()),
                            }
                        };
                        let (dot, _) =
                            ui.allocate_exact_size(egui::vec2(10.0, 16.0), Sense::hover());
                        ui.painter().circle_filled(dot.center(), 4.0, state_color);
                        ui.colored_label(state_color, state_text);
                        status_divider(ui, p);
                        // What the pane is and how big, as an icon and two numbers.
                        let backend = if s.remote.is_some() {
                            "SSH"
                        } else if matches!(s.kind, SessionKind::Serial(_)) {
                            "串口"
                        } else if cfg!(windows) {
                            "ConPTY"
                        } else {
                            "PTY"
                        };
                        status_chip(
                            ui,
                            sidebar::kind_icon(&s.kind),
                            &format!("{}×{}", s.size.1, s.size.0),
                            p,
                        )
                        .on_hover_text(format!(
                            "{} · {backend} · {} 列 × {} 行",
                            s.kind.label(),
                            s.size.1,
                            s.size.0
                        ));
                        if t.panes.len() > 1 {
                            status_chip(
                                ui,
                                icons::Icon::SplitHorizontal,
                                &format!("{}/{}", t.focused + 1, t.panes.len()),
                                p,
                            )
                            .on_hover_text("当前窗格 / 窗格总数");
                        }
                        if let Some(e) = &state.error {
                            ui.colored_label(p.danger, e);
                        }
                        if let Some(c) = &s.remote {
                            let forwards = c.forwarding.locked().clone();
                            if !forwards.is_empty() {
                                let chip = status_chip(
                                    ui,
                                    icons::Icon::Forward,
                                    &forwards.len().to_string(),
                                    p,
                                )
                                .on_hover_text("端口转发");
                                egui::Popup::menu(&chip).show(|ui| {
                                    for status in &forwards {
                                        ui.label(status);
                                    }
                                    ui.separator();
                                    if icons::icon_row(
                                        ui,
                                        icons::Icon::Close,
                                        "停止全部转发",
                                        None,
                                        p,
                                    )
                                    .clicked()
                                    {
                                        c.stop_forwards();
                                        ui.close();
                                    }
                                });
                            }
                            if icons::icon_button(ui, icons::Icon::Folder, p, icons::Size::Row)
                                .on_hover_text("文件与传输")
                                .clicked()
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
                        status_divider(ui, p);
                        ui.colored_label(p.accent, text);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
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
                        .on_hover_text(if fullscreen { "退出全屏" } else { "全屏" })
                        .clicked()
                        {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
                        }
                        if icons::icon_button(ui, icons::Icon::Settings, p, icons::Size::Row)
                            .on_hover_text(format!("偏好设置 ({})", accel("Ctrl+,")))
                            .clicked()
                        {
                            self.settings_open = true;
                        }
                        if icons::icon_button(ui, icons::Icon::Keyboard, p, icons::Size::Row)
                            .on_hover_text("快捷键")
                            .clicked()
                        {
                            self.help_open = true;
                        }
                        if let Some(link) = self.active_link() {
                            let (icon, label, next) = if link == SessionStatus::Live {
                                (
                                    icons::Icon::Disconnect,
                                    format!("断开 ({})", alt_accel("C")),
                                    Action::Disconnect,
                                )
                            } else {
                                (
                                    icons::Icon::Reconnect,
                                    format!("重新连接 ({})", alt_accel("R")),
                                    Action::Restart,
                                )
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
                        if icons::icon_button(ui, icons::Icon::Search, p, icons::Size::Row)
                            .on_hover_text(format!("查找输出 ({})", accel("Ctrl+Shift+F")))
                            .clicked()
                        {
                            self.search_open = true;
                            self.search_focus = true;
                        }
                        status_divider(ui, p);
                        let rates = format!(
                            "上行 {} · 下行 {}",
                            format_bitrate(up_rate),
                            format_bitrate(down_rate)
                        );
                        traffic(
                            ui,
                            icons::Icon::ArrowUp,
                            up_rate,
                            self.rates.up_active,
                            p.warn,
                            p,
                        )
                        .on_hover_text(&rates);
                        traffic(
                            ui,
                            icons::Icon::ArrowDown,
                            down_rate,
                            self.rates.down_active,
                            p.ok,
                            p,
                        )
                        .on_hover_text(&rates);
                        if connecting {
                            connecting_spinner(ui, p);
                        }
                    });
                });
                if let Some(state) = self.files.as_ref().and_then(Files::operation_state)
                    && self.files.as_ref().is_some_and(Files::busy)
                {
                    ui.horizontal_wrapped(|ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::hover());
                        icons::draw(ui.painter(), rect, icons::Icon::ArrowDown, p.accent, 1.4);
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
                        if icons::icon_button(ui, icons::Icon::Folder, p, icons::Size::Row)
                            .on_hover_text("打开传输窗口")
                            .clicked()
                        {
                            self.files_open = true;
                        }
                        if icons::icon_button(ui, icons::Icon::Close, p, icons::Size::Row)
                            .on_hover_text("取消传输")
                            .clicked()
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

/// A thin vertical rule between status bar groups.
fn status_divider(ui: &mut egui::Ui, p: Palette) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(9.0, 16.0), Sense::hover());
    ui.painter().line_segment(
        [rect.center_top(), rect.center_bottom()],
        egui::Stroke::new(1.0_f32, p.line),
    );
}

/// An icon followed by a short value, e.g. the terminal size.
fn status_chip(ui: &mut egui::Ui, icon: icons::Icon, text: &str, p: Palette) -> egui::Response {
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_owned(), egui::FontId::proportional(12.5), p.muted);
    let size = egui::vec2(20.0 + galley.size().x + 4.0, 20.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        if response.hovered() {
            ui.painter().rect_filled(rect, 5, p.raised);
        }
        icons::draw(
            ui.painter(),
            Rect::from_center_size(
                egui::pos2(rect.left() + 9.0, rect.center().y),
                egui::Vec2::splat(14.0),
            ),
            icon,
            p.muted,
            1.3,
        );
        ui.painter().galley(
            egui::pos2(rect.left() + 20.0, rect.center().y - galley.size().y * 0.5),
            galley,
            p.muted,
        );
    }
    response
}

/// One direction of the traffic meter: an arrow that lights up while bytes
/// move, with the rate beside it only while there is one to show.
fn traffic(
    ui: &mut egui::Ui,
    icon: icons::Icon,
    rate: f64,
    active: bool,
    lit: egui::Color32,
    p: Palette,
) -> egui::Response {
    let color = if active { lit } else { p.muted };
    let text = (rate > 0.0).then(|| format_bitrate(rate));
    let galley = text.map(|text| {
        ui.painter()
            .layout_no_wrap(text, egui::FontId::monospace(11.5), color)
    });
    let width = 16.0 + galley.as_ref().map_or(0.0, |g| g.size().x + 3.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 20.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        icons::draw(
            ui.painter(),
            Rect::from_center_size(
                egui::pos2(rect.left() + 7.0, rect.center().y),
                egui::Vec2::splat(13.0),
            ),
            icon,
            color,
            1.5,
        );
        if let Some(galley) = galley {
            ui.painter().galley(
                egui::pos2(rect.left() + 17.0, rect.center().y - galley.size().y * 0.5),
                galley,
                color,
            );
        }
    }
    response
}
