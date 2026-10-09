//! The navigation sidebar: local shells, saved connections and their groups,
//! plus the floating overlay that auto-hide mode uses.

use super::*;

pub(super) fn connection_search_id() -> egui::Id {
    egui::Id::new("connection-search")
}

#[derive(Clone, Copy)]
pub(super) enum SavedConnection {
    Ssh(usize),
    Serial(usize),
}

#[derive(Clone)]
pub(super) struct ConnectionDrag {
    pub kind: SessionKind,
    pub saved: Option<SavedConnection>,
}

pub(super) fn next_group_name(settings: &Settings) -> String {
    let last = settings
        .groups
        .iter()
        .chain(settings.profiles.iter().map(|profile| &profile.group))
        .chain(
            settings
                .serial_profiles
                .iter()
                .map(|profile| &profile.group),
        )
        .filter_map(|name| {
            name.strip_prefix("新分组 ")
                .and_then(|suffix| suffix.parse::<usize>().ok())
        })
        .max()
        .unwrap_or(0);
    format!("新分组 {}", last + 1)
}

fn drag_source(row: &egui::Response, kind: SessionKind, saved: Option<SavedConnection>) {
    if row.drag_started_by(egui::PointerButton::Primary) {
        egui::DragAndDrop::set_payload(&row.ctx, ConnectionDrag { kind, saved });
    }
}

fn can_drop_in_group(ctx: &egui::Context) -> bool {
    egui::DragAndDrop::payload::<ConnectionDrag>(ctx).is_some_and(|payload| payload.saved.is_some())
}

fn group_drop_target(
    ui: &mut egui::Ui,
    rect: Rect,
    group: Option<String>,
    action: &mut Option<Action>,
    p: Palette,
) {
    if !can_drop_in_group(ui.ctx()) || !ui.rect_contains_pointer(rect) {
        return;
    }
    ui.painter()
        .rect_filled(rect, 5, p.accent.gamma_multiply(0.10));
    ui.painter().rect_stroke(
        rect,
        5,
        egui::Stroke::new(1.0_f32, p.accent),
        egui::StrokeKind::Inside,
    );
    ui.ctx().set_cursor_icon(egui::CursorIcon::Copy);
    if ui.input(|input| input.pointer.button_released(egui::PointerButton::Primary))
        && let Some(payload) = egui::DragAndDrop::take_payload::<ConnectionDrag>(ui.ctx())
        && let Some(saved) = payload.saved
    {
        *action = Some(Action::MoveConnection(saved, group));
    }
}

pub(super) fn console_drop_target(
    ui: &mut egui::Ui,
    rect: Rect,
    action: &mut Option<Action>,
    p: Palette,
) {
    if !egui::DragAndDrop::has_payload_of_type::<ConnectionDrag>(ui.ctx())
        || !ui.rect_contains_pointer(rect)
    {
        return;
    }
    ui.ctx().set_cursor_icon(egui::CursorIcon::Copy);
    ui.painter().rect_stroke(
        rect.shrink(6.0),
        6,
        egui::Stroke::new(1.5_f32, p.accent),
        egui::StrokeKind::Inside,
    );
    let badge = Rect::from_center_size(
        egui::pos2(rect.center().x, rect.top() + 26.0),
        egui::vec2(180.0, 30.0),
    );
    ui.painter().rect_filled(badge, 5, p.raised);
    ui.painter().text(
        badge.center(),
        egui::Align2::CENTER_CENTER,
        "松开创建新会话",
        egui::FontId::proportional(14.0),
        p.accent,
    );
    if ui.input(|input| input.pointer.button_released(egui::PointerButton::Primary))
        && let Some(payload) = egui::DragAndDrop::take_payload::<ConnectionDrag>(ui.ctx())
    {
        *action = Some(Action::New(payload.kind.clone()));
    }
}

pub(super) fn drag_preview(ctx: &egui::Context, p: Palette) {
    if let Some(payload) = egui::DragAndDrop::payload::<ConnectionDrag>(ctx)
        && let Some(pointer) = ctx.pointer_interact_pos()
    {
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("connection-drag-preview"),
        ));
        let galley = painter.layout(
            payload.kind.label(),
            egui::FontId::proportional(14.0),
            p.text,
            180.0,
        );
        let rect = Rect::from_min_size(
            pointer + egui::vec2(14.0, 16.0),
            galley.size() + egui::vec2(20.0, 12.0),
        );
        painter.rect_filled(rect, 5, p.raised);
        painter.rect_stroke(
            rect,
            5,
            egui::Stroke::new(1.0_f32, p.line),
            egui::StrokeKind::Inside,
        );
        painter.galley(rect.min + egui::vec2(10.0, 6.0), galley, p.text);
    }
}

fn navigation_heading(ui: &mut egui::Ui, icon: icons::Icon, title: &str, p: Palette) {
    ui.horizontal(|ui| {
        ui.set_min_height(22.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(15.0, 15.0), Sense::hover());
        icons::draw(ui.painter(), rect, icon, p.muted, 1.3);
        ui.label(RichText::new(title).strong().color(p.text));
    });
}

fn profile_matches(profile: &RemoteProfile, filter: &str) -> bool {
    filter.is_empty()
        || [
            profile.label(),
            profile.host.clone(),
            profile.user.clone(),
            profile.group.clone(),
        ]
        .iter()
        .any(|value| value.to_lowercase().contains(filter))
}

fn connection_row(
    ui: &mut egui::Ui,
    kind: &SessionKind,
    row_id: egui::Id,
    selection: &mut Option<egui::Id>,
    p: Palette,
) -> egui::Response {
    let icon = if matches!(kind, SessionKind::Local(_) | SessionKind::Serial(_)) {
        icons::Icon::Terminal
    } else {
        icons::Icon::Host
    };
    let row = ui
        .push_id(row_id, |ui| {
            icons::navigation_row(ui, icon, &kind.label(), *selection == Some(row_id), p)
        })
        .inner;
    if row.clicked() || row.clicked_by(egui::PointerButton::Secondary) {
        *selection = Some(row_id);
    }
    row
}

fn serial_matches(profile: &SerialProfile, filter: &str) -> bool {
    filter.is_empty()
        || [profile.label(), profile.port.clone(), profile.group.clone()]
            .iter()
            .any(|value| value.to_lowercase().contains(filter))
}

impl App {
    pub(super) fn sidebar(&mut self, ctx: &egui::Context, action: &mut Option<Action>) {
        let p = self.palette;
        egui::SidePanel::left("navigation")
            .default_width(228.0)
            .width_range(180.0..=360.0)
            .frame(
                egui::Frame::new()
                    .fill(p.panel)
                    .inner_margin(egui::Margin::symmetric(8, 8)),
            )
            .show(ctx, |ui| self.sidebar_contents(ui, action));
    }
    /// The navigation bar's body, shared by the docked panel and the floating
    /// overlay that auto-hide uses.
    pub(super) fn sidebar_contents(&mut self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        let p = self.palette;
        ui.visuals_mut().indent_has_left_vline = false;
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 1.0);
        ui.spacing_mut().indent = 14.0;
        egui::ScrollArea::vertical().show(ui, |ui| {
            // Keep the last group target reachable while dragging a long list.
            if egui::DragAndDrop::has_payload_of_type::<ConnectionDrag>(ui.ctx())
                && let Some(pointer) = ui.ctx().pointer_interact_pos()
                && ui.clip_rect().contains(pointer)
            {
                let clip = ui.clip_rect();
                let delta = if pointer.y < clip.top() + 24.0 {
                    6.0
                } else if pointer.y > clip.bottom() - 24.0 {
                    -6.0
                } else {
                    0.0
                };
                if delta != 0.0 {
                    ui.scroll_with_delta(egui::vec2(0.0, delta));
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(16));
                }
            }
            navigation_heading(ui, icons::Icon::Toolbox, "工作区", p);
            ui.add_space(3.0);
            let mut submitted = false;
            let mut connect_clicked = false;
            let mut search_focused = false;
            let mut step = 0;
            let previous_filter = zeroize::Zeroizing::new(self.connection_filter.clone());
            egui::Frame::new()
                .fill(p.field)
                .stroke(egui::Stroke::new(1.0_f32, p.line))
                .corner_radius(7)
                .inner_margin(egui::Margin::symmetric(6, 4))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let quick = quick_connect::parse(&self.connection_filter).is_some();
                        let (rect, response) = ui.allocate_exact_size(
                            egui::vec2(18.0, 18.0),
                            if quick {
                                Sense::click()
                            } else {
                                Sense::hover()
                            },
                        );
                        icons::draw(
                            ui.painter(),
                            rect.shrink(2.0),
                            if quick {
                                icons::Icon::Connect
                            } else {
                                icons::Icon::Search
                            },
                            if quick { p.accent } else { p.muted },
                            1.3,
                        );
                        if quick {
                            connect_clicked = response.on_hover_text("连接此地址").clicked();
                            submitted |= connect_clicked;
                        }
                        let width = ui.available_width();
                        let search_id = connection_search_id();
                        if ui.memory(|memory| memory.has_focus(search_id))
                            && !editing::ime_composing(ui.ctx())
                        {
                            ui.input_mut(|input| {
                                if input.consume_key(egui::Modifiers::NONE, Key::ArrowDown) {
                                    step = 1;
                                }
                                if input.consume_key(egui::Modifiers::NONE, Key::ArrowUp) {
                                    step = -1;
                                }
                                submitted |= input.consume_key(egui::Modifiers::NONE, Key::Enter);
                            });
                        }
                        let response =
                            editing::field_with(ui, &mut self.connection_filter, |edit| {
                                edit.hint_text("查找连接、主机或分组")
                                    .id(search_id)
                                    .frame(false)
                                    .desired_width(width)
                            });
                        search_focused = response.has_focus();
                    });
                });
            let filter = self.connection_filter.trim().to_lowercase();
            if self.connection_filter != *previous_filter {
                self.connection_candidate = None;
            }
            let matches: Vec<_> = self
                .settings
                .profiles
                .iter()
                .chain(self.settings.ssh_config_profiles.iter())
                .filter(|profile| profile_matches(profile, &filter))
                .cloned()
                .map(SessionKind::Ssh)
                .chain(
                    self.settings
                        .serial_profiles
                        .iter()
                        .filter(|profile| serial_matches(profile, &filter))
                        .cloned()
                        .map(SessionKind::Serial),
                )
                .collect();
            if !matches.is_empty() && !filter.is_empty() && step != 0 {
                self.navigation_selection = None;
                let current = self.connection_candidate;
                self.connection_candidate = Some(if step > 0 {
                    current.map_or(0, |index| (index + 1) % matches.len())
                } else {
                    current.map_or(matches.len() - 1, |index| {
                        (index + matches.len() - 1) % matches.len()
                    })
                });
            }
            if submitted {
                if connect_clicked
                    && let Some(target) = quick_connect::parse(&self.connection_filter)
                {
                    *action = Some(Action::QuickConnect(target));
                } else if let Some(kind) = self
                    .connection_candidate
                    .and_then(|index| matches.get(index))
                {
                    *action = Some(Action::New(kind.clone()));
                    zeroize::Zeroize::zeroize(&mut self.connection_filter);
                } else if let Some(target) = quick_connect::parse(&self.connection_filter) {
                    *action = Some(Action::QuickConnect(target));
                } else if let Some(kind) = matches.first().filter(|_| !filter.is_empty()) {
                    *action = Some(Action::New(kind.clone()));
                    zeroize::Zeroize::zeroize(&mut self.connection_filter);
                }
                if action.is_some() {
                    ui.memory_mut(|memory| memory.surrender_focus(connection_search_id()));
                }
            }
            if search_focused && !filter.is_empty() && !matches.is_empty() {
                ui.add_space(4.0);
                ui.label(hint("↑↓ 选择 · Enter 连接", p));
                for (index, kind) in matches.iter().enumerate() {
                    let row = ui
                        .push_id(("connection-candidate", index), |ui| {
                            icons::navigation_row(
                                ui,
                                if matches!(kind, SessionKind::Serial(_)) {
                                    icons::Icon::Terminal
                                } else {
                                    icons::Icon::Host
                                },
                                &kind.label(),
                                self.connection_candidate == Some(index),
                                p,
                            )
                        })
                        .inner;
                    if self.connection_candidate == Some(index) && step != 0 {
                        row.scroll_to_me(Some(Align::Center));
                    }
                    if row.clicked() {
                        *action = Some(Action::New(kind.clone()));
                        zeroize::Zeroize::zeroize(&mut self.connection_filter);
                        ui.memory_mut(|memory| memory.surrender_focus(connection_search_id()));
                    }
                }
            }
            if !filter.is_empty() {
                ui.label(hint(
                    if quick_connect::parse(&self.connection_filter).is_some() {
                        "Enter 或点击图标连接此地址"
                    } else {
                        "筛选已保存连接和 SSH config"
                    },
                    p,
                ));
            }
            ui.add_space(6.0);
            egui::CollapsingHeader::new(RichText::new("本地 Shell").color(p.text))
                .default_open(true)
                .show(ui, |ui| {
                    for shell in local_shells() {
                        // Double-click, like every other row in the
                        // sidebar, so a stray click cannot open a pane.
                        let row = connection_row(
                            ui,
                            &SessionKind::Local(shell.value.clone()),
                            egui::Id::new(("navigation-shell", &shell.value)),
                            &mut self.navigation_selection,
                            p,
                        );
                        drag_source(&row, SessionKind::Local(shell.value.clone()), None);
                        if row.double_clicked() {
                            *action = Some(Action::New(SessionKind::Local(shell.value.clone())));
                        }
                        // The executable path matters on Unix, where
                        // several shells can share a name.
                        row.on_hover_text(if shell.value.contains('/') {
                            format!("双击打开 · 拖到控制台新建会话 · {}", shell.value)
                        } else {
                            "双击打开 · 拖到控制台新建会话".to_string()
                        });
                    }
                    let serial = icons::navigation_row(
                        ui,
                        icons::Icon::Host,
                        "连接串口…",
                        self.navigation_selection
                            == Some(egui::Id::new("navigation-serial-picker")),
                        p,
                    );
                    if serial.clicked() || serial.clicked_by(egui::PointerButton::Secondary) {
                        self.navigation_selection = Some(egui::Id::new("navigation-serial-picker"));
                    }
                    if serial.double_clicked() {
                        *action = Some(Action::SerialPicker);
                    }
                    serial.on_hover_text("双击打开串口选择");
                });
            if filter.is_empty() && !self.settings.recent_connections.is_empty() {
                ui.add_space(4.0);
                ui.separator();
                let mut header_clicked = false;
                let mut header = egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    egui::Id::new("recent-connections"),
                    false,
                )
                .show_header(ui, |ui| {
                    ui.set_min_height(22.0);
                    let (rect, icon) =
                        ui.allocate_exact_size(egui::vec2(15.0, 15.0), Sense::click());
                    icons::draw(ui.painter(), rect, icons::Icon::History, p.muted, 1.3);
                    let label = ui.add(
                        egui::Label::new(RichText::new("最近连接").strong().color(p.text))
                            .sense(Sense::click()),
                    );
                    header_clicked = icon.clicked() || label.clicked();
                });
                if header_clicked {
                    header.toggle();
                }
                header.body(|ui| {
                    let candidates: Vec<_> = self
                        .recent_profiles
                        .iter()
                        .cloned()
                        .chain(
                            self.settings
                                .profiles
                                .iter()
                                .chain(self.settings.ssh_config_profiles.iter())
                                .cloned()
                                .map(SessionKind::Ssh)
                                .chain(
                                    self.settings
                                        .serial_profiles
                                        .iter()
                                        .cloned()
                                        .map(SessionKind::Serial),
                                ),
                        )
                        .chain(
                            self.settings
                                .recent_connections
                                .iter()
                                .filter_map(|key| quick_connect::from_recent_key(key)),
                        )
                        .collect();
                    for key in self.settings.recent_connections.iter().take(5) {
                        if let Some(kind) = candidates
                            .iter()
                            .find(|kind| recent_connection_key(kind).as_ref() == Some(key))
                        {
                            let row = connection_row(
                                ui,
                                kind,
                                egui::Id::new(("navigation-recent", key)),
                                &mut self.navigation_selection,
                                p,
                            );
                            drag_source(&row, kind.clone(), None);
                            if row.double_clicked() {
                                *action = Some(Action::New(kind.clone()));
                            }
                            row.on_hover_text("单击选中 · 双击连接 / 右键管理")
                                .context_menu(|ui| {
                                    if ui.button("保存到连接列表").clicked() {
                                        *action = Some(Action::SaveConnection(kind.clone()));
                                        ui.close();
                                    }
                                    if ui.button("复制连接信息").clicked() {
                                        *action = Some(Action::CopyConnectionInfo(kind.clone()));
                                        ui.close();
                                    }
                                });
                        }
                    }
                });
            }
            ui.add_space(4.0);
            ui.separator();
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(15.0, 15.0), Sense::hover());
                icons::draw(ui.painter(), rect, icons::Icon::Folder, p.muted, 1.2);
                ui.label(RichText::new("所有连接").strong().color(p.text))
                    .on_hover_text("右键复制完整连接列表")
                    .context_menu(|ui| {
                        let has_connections = !self.settings.profiles.is_empty()
                            || !self.settings.serial_profiles.is_empty()
                            || !self.settings.ssh_config_profiles.is_empty();
                        if ui
                            .add_enabled(has_connections, egui::Button::new("复制连接列表"))
                            .clicked()
                        {
                            *action = Some(Action::CopyConnectionList);
                            ui.close();
                        }
                    });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icons::icon_button(ui, icons::Icon::Group, p, icons::Size::Row)
                        .on_hover_text("分组管理")
                        .clicked()
                    {
                        self.groups_open = true;
                    }
                    if icons::icon_button(ui, icons::Icon::Plus, p, icons::Size::Row)
                        .on_hover_text("新建连接")
                        .clicked()
                    {
                        *action = Some(Action::Remote);
                    }
                });
            });
            let mut groups = self.settings.groups.clone();
            for group in self
                .settings
                .profiles
                .iter()
                .map(|p| &p.group)
                .chain(self.settings.serial_profiles.iter().map(|p| &p.group))
            {
                if !group.is_empty() && !groups.contains(group) {
                    groups.push(group.clone());
                }
            }
            groups.insert(0, String::new());
            let any_profiles =
                !self.settings.profiles.is_empty() || !self.settings.serial_profiles.is_empty();
            if !any_profiles && filter.is_empty() {
                ui.label(hint("还没有保存的连接", p));
            }
            let any_match = self
                .settings
                .profiles
                .iter()
                .chain(self.settings.ssh_config_profiles.iter())
                .any(|profile| profile_matches(profile, &filter))
                || self
                    .settings
                    .serial_profiles
                    .iter()
                    .any(|profile| serial_matches(profile, &filter));
            if !filter.is_empty() && !any_match {
                ui.label(hint("没有匹配的连接", p));
            }
            for group in groups {
                let count = self
                    .settings
                    .profiles
                    .iter()
                    .filter(|p| p.group == group && profile_matches(p, &filter))
                    .count()
                    + self
                        .settings
                        .serial_profiles
                        .iter()
                        .filter(|p| p.group == group && serial_matches(p, &filter))
                        .count();
                if !filter.is_empty() && count == 0 && !can_drop_in_group(ui.ctx()) {
                    continue;
                }
                if group.is_empty() && count == 0 && !can_drop_in_group(ui.ctx()) {
                    continue;
                }
                let section = egui::CollapsingHeader::new(format!(
                    "{} ({count})",
                    if group.is_empty() {
                        "默认分组"
                    } else {
                        &group
                    }
                ))
                .id_salt((&group, "group"))
                .default_open(true)
                .show(ui, |ui| {
                    for (index, profile) in self
                        .settings
                        .profiles
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| p.group == group && profile_matches(p, &filter))
                    {
                        let r = connection_row(
                            ui,
                            &SessionKind::Ssh(profile.clone()),
                            egui::Id::new(("navigation-ssh", index)),
                            &mut self.navigation_selection,
                            p,
                        );
                        drag_source(
                            &r,
                            SessionKind::Ssh(profile.clone()),
                            Some(SavedConnection::Ssh(index)),
                        );
                        if r.double_clicked() {
                            *action = Some(Action::New(SessionKind::Ssh(profile.clone())));
                        }
                        r.on_hover_text(format!(
                            "{}:{} · 双击连接 / 右键管理 · 可拖拽分组或新建会话",
                            profile.destination(),
                            profile.port
                        ))
                        .context_menu(|ui| {
                            for (text, a) in [
                                ("连接 SSH", Action::New(SessionKind::Ssh(profile.clone()))),
                                ("编辑 / 跳板机 / 转发", Action::Edit(index)),
                                (
                                    "复制连接信息",
                                    Action::CopyConnectionInfo(SessionKind::Ssh(profile.clone())),
                                ),
                                ("复制连接配置", Action::Duplicate(index)),
                                ("删除连接", Action::Remove(index)),
                            ] {
                                if ui.button(text).clicked() {
                                    *action = Some(a);
                                    ui.close();
                                }
                            }
                        });
                    }
                    for (index, profile) in self
                        .settings
                        .serial_profiles
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| p.group == group && serial_matches(p, &filter))
                    {
                        let r = connection_row(
                            ui,
                            &SessionKind::Serial(profile.clone()),
                            egui::Id::new(("navigation-serial", index)),
                            &mut self.navigation_selection,
                            p,
                        );
                        drag_source(
                            &r,
                            SessionKind::Serial(profile.clone()),
                            Some(SavedConnection::Serial(index)),
                        );
                        if r.double_clicked() {
                            *action = Some(Action::New(SessionKind::Serial(profile.clone())));
                        }
                        r.on_hover_text(format!(
                            "串口 {} · {} bps · 双击连接 / 右键管理 · 可拖拽分组或新建会话",
                            if profile.auto() {
                                "auto".to_string()
                            } else {
                                profile.port.clone()
                            },
                            profile.baud
                        ))
                        .context_menu(|ui| {
                            for (text, a) in [
                                (
                                    "连接串口",
                                    Action::New(SessionKind::Serial(profile.clone())),
                                ),
                                ("编辑串口连接", Action::EditSerial(index)),
                                (
                                    "复制连接信息",
                                    Action::CopyConnectionInfo(SessionKind::Serial(
                                        profile.clone(),
                                    )),
                                ),
                                ("复制连接配置", Action::DuplicateSerial(index)),
                                ("删除连接", Action::RemoveSerial(index)),
                            ] {
                                if ui.button(text).clicked() {
                                    *action = Some(a);
                                    ui.close();
                                }
                            }
                        });
                    }
                });
                let rect = section
                    .body_response
                    .as_ref()
                    .map_or(section.header_response.rect, |body| {
                        section.header_response.rect.union(body.rect)
                    });
                group_drop_target(ui, rect, Some(group), action, p);
            }
            // Hosts imported from ~/.ssh/config. They live outside the
            // saved groups because the file, not settings.json, owns
            // them; "保存到连接" copies one over when it is worth keeping.
            if !self.settings.ssh_config_profiles.is_empty() {
                let imported: Vec<_> = self
                    .settings
                    .ssh_config_profiles
                    .iter()
                    .enumerate()
                    .filter(|(_, profile)| profile_matches(profile, &filter))
                    .map(|(index, profile)| (index, profile.clone()))
                    .collect();
                let count = imported.len();
                if count > 0 {
                    egui::CollapsingHeader::new(format!("SSH-CONFIG ({count})"))
                        .id_salt("ssh-config")
                        .default_open(true)
                        .show(ui, |ui| {
                            for (index, profile) in imported {
                                let r = connection_row(
                                    ui,
                                    &SessionKind::Ssh(profile.clone()),
                                    egui::Id::new(("navigation-imported", index)),
                                    &mut self.navigation_selection,
                                    p,
                                );
                                drag_source(&r, SessionKind::Ssh(profile.clone()), None);
                                if r.double_clicked() {
                                    *action = Some(Action::New(SessionKind::Ssh(profile.clone())));
                                }
                                r.on_hover_text(format!(
                                    "{}:{} · 双击连接 · 拖到控制台新建会话",
                                    profile.destination(),
                                    profile.port
                                ))
                                .context_menu(|ui| {
                                    if ui.button("连接 SSH").clicked() {
                                        *action =
                                            Some(Action::New(SessionKind::Ssh(profile.clone())));
                                        ui.close();
                                    }
                                    if ui.button("复制连接信息").clicked() {
                                        *action = Some(Action::CopyConnectionInfo(
                                            SessionKind::Ssh(profile.clone()),
                                        ));
                                        ui.close();
                                    }
                                    if ui.button("保存到连接").clicked() {
                                        *action = Some(Action::AdoptSshConfig(profile.clone()));
                                        ui.close();
                                    }
                                });
                            }
                        });
                }
            }
            if can_drop_in_group(ui.ctx()) {
                ui.add_space(4.0);
                let row = icons::navigation_row(
                    ui,
                    icons::Icon::NewFolder,
                    &format!("新建分组 · {}", next_group_name(&self.settings)),
                    false,
                    p,
                );
                group_drop_target(ui, row.rect, None, action, p);
            }
        });
    }
    /// Auto-hide mode: the bar is out of the layout and drops in from the left
    /// when the pointer reaches a slim strip at the window edge.
    pub(super) fn sidebar_overlay(&mut self, ctx: &egui::Context, action: &mut Option<Action>) {
        let p = self.palette;
        const STRIP_WIDTH: f32 = 9.0;
        const PANEL_WIDTH: f32 = 228.0;
        let avail = ctx.available_rect();
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let strip = Rect::from_min_size(
            egui::pos2(avail.left(), avail.top()),
            egui::vec2(STRIP_WIDTH, avail.height()),
        );
        let over_strip = pointer.is_some_and(|pos| strip.contains(pos));
        let over_panel = pointer.is_some_and(|pos| {
            self.sidebar_panel_rect
                .is_some_and(|rect| rect.contains(pos))
        });
        // A context menu opened from the bar counts as still hovering it.
        let over_menu = self.sidebar_reveal && egui::Popup::is_any_open(ctx);
        let reveal = over_strip || over_panel || over_menu;
        self.sidebar_reveal = reveal;
        if reveal {
            let rect = Rect::from_min_size(avail.min, egui::vec2(PANEL_WIDTH, avail.height()));
            self.sidebar_panel_rect = Some(rect);
            egui::Area::new(egui::Id::new("navigation-overlay"))
                .order(egui::Order::Foreground)
                .fixed_pos(rect.min)
                .show(ctx, |ui| {
                    ui.set_width(PANEL_WIDTH);
                    ui.set_max_height(avail.height());
                    egui::Frame::new()
                        .fill(p.panel)
                        .stroke(egui::Stroke::new(1.0_f32, p.muted.gamma_multiply(0.4)))
                        .inner_margin(5)
                        .show(ui, |ui| {
                            ui.set_width(PANEL_WIDTH - 10.0);
                            ui.set_max_height(avail.height() - 10.0);
                            self.sidebar_contents(ui, action);
                        });
                });
        } else {
            self.sidebar_panel_rect = None;
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("navigation-strip"),
            ));
            let center = egui::pos2(avail.left() + 4.0, avail.center().y);
            let tab = Rect::from_center_size(center, egui::vec2(9.0, 42.0));
            painter.rect_filled(tab, 3.0, p.panel.gamma_multiply(0.65));
            icons::draw(
                &painter,
                Rect::from_center_size(center, egui::vec2(9.0, 9.0)),
                icons::Icon::ChevronRight,
                p.muted,
                1.4,
            );
        }
    }
}
