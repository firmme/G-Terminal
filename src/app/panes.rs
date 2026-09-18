//! The pane area: the split layout, the panes themselves, and the chooser that
//! asks which session a fresh split should hold.

use super::*;

impl App {
    /// The little window a fresh split raises while it asks which session to
    /// put there; closing it keeps the auto-spawned one.
    pub(super) fn split_chooser(&mut self, ctx: &egui::Context, p: Palette) {
        if let Some((tab_id, pane_id)) = self.split_chooser {
            let exists = self
                .tabs
                .iter()
                .any(|t| t.id == tab_id && t.panes.iter().any(|p| p.id == pane_id));
            if !exists {
                self.split_chooser = None;
            } else {
                let mut open = true;
                let mut chosen: Option<SessionKind> = None;
                let mut keep = false;
                egui::Window::new("选择新窗格会话")
                    .open(&mut open)
                    .collapsible(false)
                    .resizable(false)
                    .default_width(260.0)
                    .show(ctx, |ui| {
                        ui.label(hint(
                            "已自动开启一个相同类型的会话，可选择其他会话替换。",
                            p,
                        ));
                        for shell in local_shells() {
                            if ui.button(format!("本地 {}", shell.label)).clicked() {
                                chosen = Some(SessionKind::Local(shell.value.clone()));
                            }
                        }
                        ui.separator();
                        for profile in &self.settings.profiles {
                            if ui.button(format!("SSH · {}", profile.label())).clicked() {
                                chosen = Some(SessionKind::Ssh(profile.clone()));
                            }
                        }
                        for profile in &self.settings.serial_profiles {
                            if ui.button(format!("串口 · {}", profile.label())).clicked() {
                                chosen = Some(SessionKind::Serial(profile.clone()));
                            }
                        }
                        if ui.button("保持当前会话").clicked() {
                            keep = true;
                        }
                    });
                if !open || keep {
                    // Closing the chooser keeps the auto-spawned session.
                    self.split_chooser = None;
                }
                if let Some(kind) = chosen {
                    let target = self
                        .tabs
                        .iter()
                        .position(|t| t.id == tab_id)
                        .and_then(|ti| {
                            self.tabs[ti]
                                .panes
                                .iter()
                                .position(|p| p.id == pane_id)
                                .map(|pi| (ti, pi))
                        });
                    if let Some((ti, pi)) = target {
                        match kind {
                            SessionKind::Ssh(profile) => {
                                self.login_target = Some((tab_id, pane_id));
                                self.announce(
                                    tab_id,
                                    pane_id,
                                    Some(&format!("{}:{}", profile.destination(), profile.port)),
                                );
                                let console = self.terminal_of(tab_id, pane_id);
                                self.login = Some(remote_ui::Login::new(profile, console));
                            }
                            kind => {
                                let previous = self.tabs[ti].panes[pi].session.terminal.clone();
                                if let Some(pane) = self.respawn(kind, Some(previous), ctx) {
                                    self.tabs[ti].panes[pi] = pane;
                                    self.tabs[ti].focused = pi;
                                }
                            }
                        }
                    }
                    self.split_chooser = None;
                }
            }
        }
    }

    /// Draws every pane of the active tab and returns the notice a pane raised.
    pub(super) fn panes(
        &mut self,
        ctx: &egui::Context,
        action: &mut Option<Action>,
        p: Palette,
    ) -> Option<String> {
        // Collected inside the pane loop because the pane borrows the tab list,
        // and applied by the caller because the toast lives on the app.
        let mut notice = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg).inner_margin(0))
            .show(ctx, |ui| {
                if self.search_open {
                    ui.horizontal(|ui| {
                        ui.label(hint("历史", p));
                        let r = editing::field_with(ui, &mut self.search, |edit| {
                            edit.desired_width(260.0)
                        });
                        if self.search_focus {
                            r.request_focus();
                            self.search_focus = false;
                        }
                        if ui.button("查找").clicked()
                            || (r.has_focus()
                                && !editing::ime_composing(ui.ctx())
                                && ui.input(|i| i.key_pressed(Key::Enter)))
                        {
                            self.find(false);
                        }
                        if ui.button("下一处").clicked() {
                            self.find(true);
                        }
                        ui.label(hint(
                            &format!(
                                "{} / {}",
                                if self.search_hits.is_empty() {
                                    0
                                } else {
                                    self.search_index + 1
                                },
                                self.search_hits.len()
                            ),
                            p,
                        ));
                        if ui.small_button("×").clicked() {
                            self.search_open = false;
                            r.surrender_focus();
                        }
                    });
                }
                let keyboard = !self.settings_open
                    && !self.remote_open
                    && !self.groups_open
                    && !self.help_open
                    && !self.update_open
                    && !self.search_open
                    && self.login.is_none()
                    && !self.files_open
                    && self.toolbox.is_none()
                    && !egui::Popup::is_any_open(ctx);
                if let Some(t) = self.tabs.get_mut(self.active) {
                    let mut rects = vec![];
                    let rect = ui.available_rect_before_wrap();
                    layout_rects(&mut t.layout, rect, ui, t.id, 1, &mut rects);
                    let mut focus = t.focused;
                    for (index, rect) in rects {
                        ui.scope_builder(
                            egui::UiBuilder::new().id_salt((t.id, index)).max_rect(rect),
                            |ui| {
                                let pane = &mut t.panes[index];
                                if pane.session.status.lock().unwrap().exit_code.is_some() {
                                    ui.horizontal(|ui| {
                                        ui.label(hint("会话已结束", p));
                                        if ui.small_button("重连 / 重启").clicked() {
                                            focus = index;
                                            *action = Some(Action::Restart);
                                        }
                                    });
                                }
                                let (clicked, error) = pane.show(
                                    ui,
                                    crate::view::ViewOptions {
                                        active: index == t.focused,
                                        keyboard_enabled: keyboard,
                                        size: self.settings.font_size,
                                        palette: p,
                                        query: if self.search_open { &self.search } else { "" },
                                        copy_on_select: self.settings.copy_on_select,
                                        search_engine: &self.settings.search_engine,
                                    },
                                );
                                if clicked {
                                    focus = index;
                                }
                                if error.is_some() {
                                    self.error = error;
                                }
                                if let Some(text) = pane.notice.take() {
                                    notice = Some(text);
                                }
                            },
                        );
                    }
                    t.focused = focus;
                } else {
                    ui.centered_and_justified(|ui| {
                        if ui
                            .button(format!("+ 新建终端   {}", accel("Ctrl+Shift+T")))
                            .clicked()
                        {
                            *action = Some(Action::New(SessionKind::Local(
                                self.settings.default_shell.clone(),
                            )));
                        }
                    });
                }
            });
        notice
    }
}

fn layout_rects(
    layout: &mut PaneLayout,
    rect: Rect,
    ui: &mut egui::Ui,
    tab: u64,
    path: u64,
    out: &mut Vec<(usize, Rect)>,
) {
    match layout {
        PaneLayout::Leaf(i) => out.push((*i, rect)),
        PaneLayout::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let extent = if *axis == Axis::Horizontal {
                rect.width()
            } else {
                rect.height()
            };
            let min = (80.0 / extent.max(1.0)).min(0.45);
            *ratio = ratio.clamp(min, 1.0 - min);
            let at = extent * *ratio;
            let (a, b, divider) = if *axis == Axis::Horizontal {
                (
                    Rect::from_min_max(rect.min, egui::pos2(rect.left() + at - 2.0, rect.bottom())),
                    Rect::from_min_max(egui::pos2(rect.left() + at + 2.0, rect.top()), rect.max),
                    Rect::from_min_max(
                        egui::pos2(rect.left() + at - 2.0, rect.top()),
                        egui::pos2(rect.left() + at + 2.0, rect.bottom()),
                    ),
                )
            } else {
                (
                    Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.top() + at - 2.0)),
                    Rect::from_min_max(egui::pos2(rect.left(), rect.top() + at + 2.0), rect.max),
                    Rect::from_min_max(
                        egui::pos2(rect.left(), rect.top() + at - 2.0),
                        egui::pos2(rect.right(), rect.top() + at + 2.0),
                    ),
                )
            };
            let r = ui.interact(divider, ui.id().with((tab, path)), Sense::drag());
            if r.dragged()
                && let Some(pos) = r.interact_pointer_pos()
            {
                *ratio = if *axis == Axis::Horizontal {
                    (pos.x - rect.left()) / extent
                } else {
                    (pos.y - rect.top()) / extent
                };
            }
            r.on_hover_cursor(if *axis == Axis::Horizontal {
                egui::CursorIcon::ResizeHorizontal
            } else {
                egui::CursorIcon::ResizeVertical
            });
            layout_rects(first, a, ui, tab, path * 2, out);
            layout_rects(second, b, ui, tab, path * 2 + 1, out);
        }
    }
}
