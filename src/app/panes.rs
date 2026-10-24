//! The pane area: the split layout, the panes themselves, and the chooser that
//! asks which session a fresh split should hold.

use super::*;
use crate::icons::Icon;

impl App {
    /// The little window a fresh split raises while it asks which session to
    /// put there; closing it keeps the auto-spawned one.
    pub(super) fn split_chooser(&mut self, ctx: &egui::Context, p: Palette) {
        let focus = editing::window_focus(ctx, "split-chooser", self.split_chooser.is_some());
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
                crate::dialog::Dialog::new("选择新窗格会话", Icon::SplitHorizontal, p)
                    .open(&mut open)
                    .collapsible(false)
                    .resizable(false)
                    .default_width(360.0)
                    .show(ctx, |ui| {
                        ui.label(hint("新窗格已打开相同会话，可换成：", p));
                        ui.add_space(2.0);
                        let mut kinds: Vec<SessionKind> = local_shells()
                            .iter()
                            .map(|shell| SessionKind::Local(shell.value.clone()))
                            .collect();
                        kinds.extend(self.settings.profiles.iter().cloned().map(SessionKind::Ssh));
                        kinds.extend(
                            self.settings
                                .serial_profiles
                                .iter()
                                .cloned()
                                .map(SessionKind::Serial),
                        );
                        egui::ScrollArea::vertical()
                            .max_height(320.0)
                            .show(ui, |ui| {
                                for (index, kind) in kinds.into_iter().enumerate() {
                                    let row = ui
                                        .push_id(index, |ui| {
                                            icons::navigation_row(
                                                ui,
                                                sidebar::kind_icon(&kind),
                                                &kind.label(),
                                                false,
                                                connection_color(
                                                    &kind,
                                                    &self.settings.group_colors,
                                                ),
                                                p,
                                            )
                                        })
                                        .inner;
                                    if index == 0 {
                                        focus.request(ui, &row);
                                    }
                                    if row.clicked() {
                                        chosen = Some(kind);
                                    }
                                }
                            });
                        crate::dialog::footer(ui, |ui| {
                            if crate::dialog::secondary(ui, "保持不变").clicked() {
                                keep = true;
                            }
                        });
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
                    egui::Frame::new()
                        .fill(p.panel)
                        .inner_margin(egui::Margin::symmetric(10, 4))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                let field = ui.allocate_ui(egui::vec2(300.0, 30.0), |ui| {
                                    let r = crate::dialog::icon_field(
                                        ui,
                                        icons::Icon::Search,
                                        "查找输出（含历史）",
                                        &mut self.search,
                                        false,
                                        p,
                                    );
                                    if self.search_focus {
                                        r.request_focus();
                                        self.search_focus = false;
                                    }
                                    if r.has_focus()
                                        && !editing::ime_composing(ui.ctx())
                                        && ui.input(|i| i.key_pressed(Key::Enter))
                                    {
                                        self.find(false);
                                    }
                                    r.has_focus() || r.lost_focus()
                                });
                                let field_focused = field.inner;
                                let found = !self.search_hits.is_empty();
                                ui.label(
                                    RichText::new(format!(
                                        "{} / {}",
                                        if found { self.search_index + 1 } else { 0 },
                                        self.search_hits.len()
                                    ))
                                    .monospace()
                                    .color(
                                        if found || self.search.is_empty() {
                                            p.muted
                                        } else {
                                            p.danger
                                        },
                                    ),
                                );
                                if icons::icon_button(ui, icons::Icon::Search, p, icons::Size::Row)
                                    .on_hover_text("查找 (Enter)")
                                    .clicked()
                                {
                                    self.find(false);
                                }
                                if icons::icon_button(
                                    ui,
                                    icons::Icon::ArrowDown,
                                    p,
                                    icons::Size::Row,
                                )
                                .on_hover_text("下一处")
                                .clicked()
                                {
                                    self.find(true);
                                }
                                // Esc only while typing here: in the terminal it
                                // belongs to the running program.
                                if icons::close_button(ui, p, "关闭查找 (Esc)").clicked()
                                    || (field_focused && ui.input(|i| i.key_pressed(Key::Escape)))
                                {
                                    self.search_open = false;
                                }
                            });
                        });
                }
                let workspace_available = !self.settings_open
                    && !self.remote_open
                    && !self.groups_open
                    && !self.help_open
                    && !self.update_open
                    && self.login.is_none()
                    && !self.files_open
                    && !self.files.as_ref().is_some_and(Files::has_modal_dialog)
                    && self.toolbox.is_none()
                    && self.serial_picker.is_none()
                    && self.port_owner.is_none()
                    && self.split_chooser.is_none()
                    && !self.confirm_exit
                    && !egui::Popup::is_any_open(ctx);
                let keyboard = workspace_available
                    && !self.search_open
                    && !ctx.memory(|memory| memory.has_focus(sidebar::connection_search_id()))
                    && !egui::DragAndDrop::has_payload_of_type::<sidebar::ConnectionDrag>(ctx);
                let console_rect = ui.available_rect_before_wrap();
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
                if workspace_available {
                    sidebar::console_drop_target(ui, console_rect, action, p);
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
