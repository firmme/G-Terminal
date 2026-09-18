//! The status bar: what the focused session is doing, the traffic meters, the
//! connecting spinner and the frameless window's resize grip.

use super::*;

impl App {
    /// Turns the byte counters into rates once a frame, keeps the spinner
    /// and the settling rates repainting, and lets a toast expire.
    pub(super) fn tick_status(&mut self, ctx: &egui::Context) {
        // The counters become rates here, once per frame. Both the spinner and
        // a settling rate need more frames than an idle app would ask for.
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

    /// The bottom strip: session state on the left, meters and the resize
    /// grip on the right.
    pub(super) fn status_bar(
        &mut self,
        ctx: &egui::Context,
        action: &mut Option<Action>,
        p: Palette,
    ) {
        let connecting = self.connecting();
        let down_rate = self.rates.down_per_sec;
        let up_rate = self.rates.up_per_sec;
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(p.panel)
                    .inner_margin(egui::Margin::symmetric(6, 2)),
            )
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
                    if let Some(t) = self.tabs.get(self.active) {
                        let s = &t.panes[t.focused].session;
                        let state = s.status.lock().unwrap();
                        ui.colored_label(
                            p.accent,
                            if let Some(code) = state.exit_code {
                                format!("已退出 {code}")
                            } else {
                                "● 运行中".into()
                            },
                        );
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
                        // A frameless window has no OS grip, so the corner is
                        // drawn and made draggable here. macOS resizes through
                        // its native border instead.
                        if !cfg!(target_os = "macos") {
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
                        }
                        if connecting {
                            connecting_spinner(ui, p);
                        }
                        // Two arrows rather than a number: they are always
                        // there, so the corner does not jump around, and the
                        // colour says whether anything is moving. Down is green
                        // and up is red; the exact figures are on hover.
                        let rates = format!(
                            "下行 {}\n上行 {}",
                            format_bitrate(down_rate),
                            format_bitrate(up_rate)
                        );
                        ui.label(RichText::new("↓").color(if down_rate > TRAFFIC_IDLE {
                            p.ok
                        } else {
                            p.muted
                        }))
                        .on_hover_text(&rates);
                        ui.label(RichText::new("↑").color(if up_rate > TRAFFIC_IDLE {
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
                        ui.label(hint("UTF-8", p));
                    });
                });
                // A terminal-initiated transfer runs behind the terminal, so its
                // progress has to be visible without the file window.
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
    }
}

/// How long a status-bar message stays on screen.
pub(super) const TOAST_LIFETIME: std::time::Duration = std::time::Duration::from_millis(2500);

/// Turns a pane's byte counters into a smoothed per-second rate. The counters
/// are cumulative, so a rate is the difference since the last sample.
#[derive(Default)]
pub(super) struct RateMeter {
    pane: Option<u64>,
    at: Option<std::time::Instant>,
    up: u64,
    down: u64,
    pub(super) up_per_sec: f64,
    pub(super) down_per_sec: f64,
}

/// Traffic this quiet counts as idle, so the arrows settle back to grey.
pub(super) const TRAFFIC_IDLE: f64 = 1.0;

impl RateMeter {
    /// Samples a pane, returning whether the rates are still worth animating.
    ///
    /// Every frame samples, rather than waiting for a fixed interval: a short
    /// burst has to light the arrows on the frames it causes, and those are the
    /// only frames guaranteed to happen.
    pub(super) fn sample(&mut self, pane: u64, up: u64, down: u64) -> bool {
        let now = std::time::Instant::now();
        if self.pane == Some(pane) {
            let seconds = self
                .at
                .map(|at| now.duration_since(at).as_secs_f64())
                .unwrap_or(0.0)
                .max(0.001);
            let up_rate = up.saturating_sub(self.up) as f64 / seconds;
            let down_rate = down.saturating_sub(self.down) as f64 / seconds;
            // Averaging over the last few samples keeps the arrows from
            // flickering while still falling back to idle when traffic stops.
            self.up_per_sec = self.up_per_sec * 0.5 + up_rate * 0.5;
            self.down_per_sec = self.down_per_sec * 0.5 + down_rate * 0.5;
        } else {
            self.pane = Some(pane);
            self.up_per_sec = 0.0;
            self.down_per_sec = 0.0;
        }
        self.at = Some(now);
        self.up = up;
        self.down = down;
        self.up_per_sec > TRAFFIC_IDLE || self.down_per_sec > TRAFFIC_IDLE
    }

    pub(super) fn clear(&mut self) {
        let pane = self.pane;
        *self = Self {
            pane,
            ..Self::default()
        };
    }
}

/// Bytes per second as an adaptive bit rate, which is how a link is described.
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

/// Eight small squares in a 3x3 ring, chasing each other round while a
/// connection is being made.
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
        // How far behind the head this square is, 0 at the head.
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

/// The corner grip a frameless window would otherwise have to answer for with a
/// few invisible pixels. Three diagonal ticks, brighter under the pointer.
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
