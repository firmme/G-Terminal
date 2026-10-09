//! The titlebar: the frameless window's chrome, the brand menu and the tab strip.

use super::*;

impl App {
    pub(super) fn resize_grips(&self, ctx: &egui::Context) {
        if ctx.input(|i| {
            i.viewport().fullscreen.unwrap_or(false) || i.viewport().maximized.unwrap_or(false)
        }) {
            return;
        }
        use egui::viewport::ResizeDirection;
        const GRIP: f32 = 5.0;
        let screen = ctx.content_rect();
        let Some(pointer) = ctx.input(|i| i.pointer.hover_pos()) else {
            return;
        };
        let direction = match (
            pointer.x - screen.left() <= GRIP,
            screen.right() - pointer.x <= GRIP,
            pointer.y - screen.top() <= GRIP,
            screen.bottom() - pointer.y <= GRIP,
        ) {
            (true, _, true, _) => Some(ResizeDirection::NorthWest),
            (_, true, true, _) => Some(ResizeDirection::NorthEast),
            (true, _, _, true) => Some(ResizeDirection::SouthWest),
            (_, true, _, true) => Some(ResizeDirection::SouthEast),
            (true, _, _, _) => Some(ResizeDirection::West),
            (_, true, _, _) => Some(ResizeDirection::East),
            (_, _, true, _) => Some(ResizeDirection::North),
            (_, _, _, true) => Some(ResizeDirection::South),
            _ => None,
        };
        let Some(direction) = direction else {
            return;
        };
        ctx.set_cursor_icon(match direction {
            ResizeDirection::North | ResizeDirection::South => egui::CursorIcon::ResizeVertical,
            ResizeDirection::East | ResizeDirection::West => egui::CursorIcon::ResizeHorizontal,
            ResizeDirection::NorthWest | ResizeDirection::SouthEast => egui::CursorIcon::ResizeNwSe,
            ResizeDirection::NorthEast | ResizeDirection::SouthWest => egui::CursorIcon::ResizeNeSw,
        });
        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
        }
    }

    pub(super) fn topbar(&mut self, ctx: &egui::Context, action: &mut Option<Action>) {
        let p = self.palette;
        egui::TopBottomPanel::top("topbar")
            .frame(
                egui::Frame::new()
                    .fill(p.panel)
                    .inner_margin(topbar_margin()),
            )
            .show(ctx, |ui| {
                let mut close = false;
                let mut toggle = false;
                let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                let custom_chrome = !cfg!(target_os = "macos");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let buttons_left = if custom_chrome {
                        let close_button =
                            title_button(ui, TitleButton::Close, p).on_hover_text("关闭");
                        let toggle_button = title_button(
                            ui,
                            if maximized {
                                TitleButton::Restore
                            } else {
                                TitleButton::Maximize
                            },
                            p,
                        )
                        .on_hover_text("最大化 / 还原");
                        let minimize_button =
                            title_button(ui, TitleButton::Minimize, p).on_hover_text("最小化");
                        if close_button.clicked() {
                            close = true;
                        }
                        if toggle_button.clicked() {
                            toggle = true;
                        }
                        if minimize_button.clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }
                        // The drag region must stop before the window buttons.
                        close_button
                            .rect
                            .union(toggle_button.rect)
                            .union(minimize_button.rect)
                            .left()
                            - 4.0
                    } else {
                        ui.max_rect().right()
                    };
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        let capture_menu = self.screenshot.is_some()
                            && std::env::var("GTERMINAL_SCREENSHOT_VIEW").as_deref() == Ok("menu");
                        brand_menu_button(ui, p.accent, capture_menu, |ui| {
                            menu_heading(ui, "新建", p);
                            if icons::icon_row_primary(
                                ui,
                                icons::Icon::Terminal,
                                "新建终端",
                                Some(accel("Ctrl+Shift+T").as_str()),
                                p,
                            )
                            .clicked()
                            {
                                *action = Some(Action::New(SessionKind::Local(
                                    self.settings.default_shell.clone(),
                                )));
                                ui.close();
                            }
                            for (icon, text, a) in [
                                (icons::Icon::Host, "新建 SSH 连接", Action::Remote),
                                (
                                    icons::Icon::Terminal,
                                    "连接串口",
                                    Action::SerialPicker,
                                ),
                            ] {
                                if icons::icon_row(ui, icon, text, None, p).clicked() {
                                    *action = Some(a);
                                    ui.close();
                                }
                            }
                            if icons::icon_row(ui, icons::Icon::Terminal, "新建窗口", None, p)
                                .clicked()
                            {
                                *action = Some(Action::NewWindow);
                                ui.close();
                            }
                            ui.separator();
                            menu_heading(ui, "当前会话", p);
                            let live = self.active_link() == Some(SessionStatus::Live);
                            if live
                                && icons::icon_row(
                                    ui,
                                    icons::Icon::Disconnect,
                                    "断开当前连接",
                                    Some(alt_accel("C").as_str()),
                                    p,
                                )
                                .clicked()
                            {
                                *action = Some(Action::Disconnect);
                                ui.close();
                            }
                            if (!live || !self.active_is_remote())
                                && icons::icon_row(
                                    ui,
                                    icons::Icon::Reconnect,
                                    if live {
                                        "重新连接 / 重启"
                                    } else {
                                        "重新连接"
                                    },
                                    Some(alt_accel("R").as_str()),
                                    p,
                                )
                                .clicked()
                            {
                                *action = Some(Action::Restart);
                                ui.close();
                            }
                            if icons::icon_row(
                                ui,
                                icons::Icon::Search,
                                "查找历史",
                                Some(accel("Ctrl+Shift+F").as_str()),
                                p,
                            )
                            .clicked()
                            {
                                self.search_open = true;
                                self.search_focus = true;
                                ui.close();
                            }
                            ui.menu_button("分屏与窗格", |ui| {
                                for (icon, text, shortcut, a) in [
                                    (
                                        icons::Icon::SplitHorizontal,
                                        "左右分屏",
                                        Some(accel("Ctrl+Shift+D")),
                                        Action::Split(Axis::Horizontal),
                                    ),
                                    (
                                        icons::Icon::SplitVertical,
                                        "上下分屏",
                                        Some(accel("Ctrl+Shift+E")),
                                        Action::Split(Axis::Vertical),
                                    ),
                                    (
                                        icons::Icon::ClosePane,
                                        "关闭窗格",
                                        Some(accel("Ctrl+Shift+W")),
                                        Action::ClosePane,
                                    ),
                                ] {
                                    if icons::icon_row(ui, icon, text, shortcut.as_deref(), p)
                                        .clicked()
                                    {
                                        *action = Some(a);
                                        ui.close();
                                    }
                                }
                            });
                            ui.separator();
                            menu_heading(ui, "文件与连接", p);
                            if icons::icon_row(ui, icons::Icon::File, "文件与传输队列", None, p)
                                .clicked()
                            {
                                *action = Some(Action::Files);
                                ui.close();
                            }
                            ui.menu_button("连接管理与工具", |ui| {
                                if icons::icon_row(ui, icons::Icon::Group, "连接分组管理", None, p)
                                    .clicked()
                                {
                                    self.groups_open = true;
                                    ui.close();
                                }
                                if icons::icon_row(ui, icons::Icon::Host, "串口占用排查", None, p)
                                    .clicked()
                                {
                                    *action = Some(Action::FindPortOwner(self.port_owner_default()));
                                    ui.close();
                                }
                                if icons::icon_row(ui, icons::Icon::Toolbox, "服务器工具箱", None, p)
                                    .clicked()
                                {
                                    *action = Some(Action::Toolbox);
                                    ui.close();
                                }
                            });
                            ui.separator();
                            menu_heading(ui, "应用", p);
                            if ui.checkbox(&mut self.settings.sidebar, "显示导航栏").changed() {
                                ui.close();
                            }
                            if icons::icon_row(
                                ui,
                                icons::Icon::Settings,
                                "偏好设置",
                                Some(accel("Ctrl+,").as_str()),
                                p,
                            )
                            .clicked()
                            {
                                self.settings_open = true;
                                ui.close();
                            }
                            ui.menu_button("帮助与更新", |ui| {
                                if icons::icon_row(ui, icons::Icon::Help, "快捷键 / 关于", None, p)
                                    .clicked()
                                {
                                    self.help_open = true;
                                    ui.close();
                                }
                                if icons::icon_row(ui, icons::Icon::Refresh, "检查更新", None, p)
                                    .clicked()
                                {
                                    *action = Some(Action::CheckUpdates);
                                    ui.close();
                                }
                            });
                            ui.separator();
                            ui.label(hint(concat!("G-Terminal ", env!("CARGO_PKG_VERSION")), p));
                            if icons::icon_row(ui, icons::Icon::Restart, "退出", None, p).clicked() {
                                *action = Some(Action::Exit);
                                ui.close();
                            }
                        });
                        let auto_hide = self.settings.auto_hide_sidebar;
                        let docked = self.settings.sidebar && (!auto_hide || self.sidebar_pinned);
                        let (icon, hover) = if auto_hide {
                            if docked {
                                (icons::Icon::ChevronLeft, "改为自动隐藏")
                            } else {
                                (icons::Icon::ChevronRight, "固定展开导航栏")
                            }
                        } else if self.settings.sidebar {
                            (icons::Icon::ChevronLeft, "收起导航栏")
                        } else {
                            (icons::Icon::ChevronRight, "展开导航栏")
                        };
                        if icons::icon_button(ui, icon, p, icons::Size::Button)
                            .on_hover_text(hover)
                            .clicked()
                        {
                            if auto_hide {
                                if self.settings.sidebar {
                                    self.sidebar_pinned = !self.sidebar_pinned;
                                } else {
                                    self.settings.sidebar = true;
                                    self.sidebar_pinned = true;
                                }
                            } else {
                                self.settings.sidebar = !self.settings.sidebar;
                            }
                        }
                        ui.separator();
                        const DRAG_GAP: f32 = 60.0;
                        let strip_max = (buttons_left - ui.cursor().min.x - DRAG_GAP).max(120.0);
                        let cursor = ui.cursor().min;
                        let strip_rect = egui::Rect::from_min_max(
                            cursor,
                            egui::pos2(cursor.x + strip_max, cursor.y + 34.0),
                        );
                        if ui.rect_contains_pointer(strip_rect) {
                            // Convert wheel movement only while hovering the tab strip.
                            ui.ctx().input_mut(|input| {
                                let wheel = input.smooth_scroll_delta.y;
                                input.smooth_scroll_delta.x += wheel;
                                input.smooth_scroll_delta.y = 0.0;
                            });
                        }
                        ui.scope(|ui| {
                            let scroll = &mut ui.style_mut().spacing.scroll;
                            scroll.bar_width = 3.0;
                            scroll.bar_inner_margin = 0.0;
                            scroll.bar_outer_margin = 0.0;
                            egui::ScrollArea::horizontal()
                                .id_salt("tab-strip")
                                .max_width(strip_max)
                                .scroll_bar_visibility(
                                    egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded,
                                )
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        for (i, t) in self.tabs.iter().enumerate() {
                                            let updated = tab_updated(t, i == self.active);
                                            ui.push_id(t.id, |ui| {
                                                let hit_id = ui.id().with("tab-hit");
                                                let hovered = ui
                                                    .ctx()
                                                    .read_response(hit_id)
                                                    .is_some_and(|r| r.hovered());
                                                let active = i == self.active;
                                                let base = if active { p.raised } else { p.panel };
                                                let (fill, stroke) = match connection_color(
                                                    &t.panes[t.focused].session.kind,
                                                    &self.settings.group_colors,
                                                ) {
                                                    Some(color) => (
                                                        blend(
                                                            base,
                                                            color,
                                                            if active {
                                                                0.20
                                                            } else if hovered {
                                                                0.22
                                                            } else {
                                                                0.15
                                                            },
                                                        ),
                                                        if active {
                                                            egui::Stroke::new(
                                                                1.0_f32,
                                                                color.gamma_multiply(0.6),
                                                            )
                                                        } else {
                                                            egui::Stroke::new(1.0_f32, p.line)
                                                        },
                                                    ),
                                                    None if active => (
                                                        p.raised,
                                                        egui::Stroke::new(
                                                            1.0_f32,
                                                            p.line,
                                                        ),
                                                    ),
                                                    None if hovered => (
                                                        blend(p.panel, p.accent, 0.10),
                                                        egui::Stroke::new(1.0_f32, p.line),
                                                    ),
                                                    None => (blend(p.panel, p.raised, 0.35), egui::Stroke::new(1.0_f32, p.line)),
                                                };
                                                // Frame strokes contribute to layout size. Keep the
                                                // width fixed when only the hover appearance changes.
                                                let tab_frame = egui::Frame::new()
                                                    .fill(fill)
                                                    .stroke(stroke)
                                                    .corner_radius(egui::CornerRadius { nw: 7, ne: 7, sw: 0, se: 0 })
                                                    .inner_margin(egui::Margin::symmetric(10, 4))
                                                    .show(ui, |ui| {
                                                        let mut hit = None;
                                                        ui.horizontal(|ui| {
                                                            let reconnect = t.panes.iter().any(|pane| !pane.session.pending() && pane.session.link() != SessionStatus::Live)
                                                                && self.login_target.is_none_or(|(id, _)| id != t.id);
                                                            let (dot, state_response) = ui.allocate_exact_size(
                                                                egui::vec2(16.0, 16.0),
                                                                if reconnect || !active { Sense::click() } else { Sense::hover() },
                                                            );
                                                            if reconnect && state_response.hovered() {
                                                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                                                icons::draw(ui.painter(), dot.shrink(1.0), icons::Icon::Reconnect, p.accent, 1.3);
                                                            } else {
                                                            ui.painter().circle_filled(
                                                                dot.center(),
                                                                4.5,
                                                                link_color(tab_link(t), p),
                                                            );
                                                            }
                                                            if state_response.clicked() && reconnect {
                                                                *action = Some(Action::ReconnectTab(i));
                                                            } else if state_response.clicked() && !active {
                                                                *action = Some(Action::ActivateTab(i));
                                                            }
                                                            state_response.on_hover_text(if reconnect { "点击重新连接" } else { tab_link(t).describe() });
                                                            for (index, pane) in
                                                                t.panes.iter().enumerate()
                                                            {
                                                                if index > 0 {
                                                                    ui.label(
                                                                        RichText::new("|")
                                                                            .color(p.muted),
                                                                    );
                                                                }
                                                                let focused = index == t.focused;
                                                                let color = if i == self.active {
                                                                    if focused {
                                                                        p.text
                                                                    } else {
                                                                        p.muted
                                                                    }
                                                                } else {
                                                                    p.muted
                                                                };
                                                                let name = pane.session.kind.label();
                                                                let text: String = name.chars().take(22).collect();
                                                                let text = if name.chars().count() > 22 {
                                                                    format!("{text}…")
                                                                } else {
                                                                    text
                                                                };
                                                                let label = RichText::new(text)
                                                                    .color(color);
                                                                let label = if focused {
                                                                    label.strong()
                                                                } else {
                                                                    label
                                                                };
                                                                ui.label(if updated {
                                                                    label.underline()
                                                                } else {
                                                                    label
                                                                });
                                                                if active && focused && pane.session.pending() {
                                                                    ui.label(RichText::new("连接中").small().color(p.muted));
                                                                }
                                                            }
                                                            if i != self.active
                                                                && t.panes.iter().any(|pane| {
                                                                    pane.session.traffic.bell.load(
                                                                        std::sync::atomic::Ordering::Relaxed,
                                                                    )
                                                                })
                                                            {
                                                                let (bell, _) = ui
                                                                    .allocate_exact_size(
                                                                        egui::vec2(12.0, 12.0),
                                                                        Sense::hover(),
                                                                    );
                                                                icons::draw(
                                                                    ui.painter(),
                                                                    bell,
                                                                    icons::Icon::Bell,
                                                                    p.warn,
                                                                    1.2,
                                                                );
                                                            }
                                                            let close = icons::glyph_button(
                                                                ui,
                                                                "×",
                                                                p,
                                                                "关闭标签",
                                                            );
                                                            if close.clicked() {
                                                                *action = Some(Action::CloseTab(i));
                                                            }
                                                            let label_rect = ui.min_rect();
                                                            hit = Some(egui::Rect::from_min_max(
                                                                egui::pos2(label_rect.left() + 16.0 + ui.spacing().item_spacing.x, label_rect.top()),
                                                                egui::pos2(
                                                                    close.rect.left(),
                                                                    label_rect.max.y,
                                                                ),
                                                            ));
                                                        });
                                                        let Some(rect) = hit else { return };
                                                            let full_title = t.panes[t.focused]
                                                                .session
                                                                .terminal
                                                                .lock()
                                                                .unwrap_or_else(|e| e.into_inner())
                                                                .title()
                                                                .to_string();
                                                            let r = ui
                                                                .interact(rect, hit_id, Sense::click())
                                                                .on_hover_text(format!(
                                                                "{}{} · {} · {} 个窗格 · 中键关闭",
                                                                t.panes[t.focused]
                                                                    .session
                                                                    .kind
                                                                    .label(),
                                                                if full_title.is_empty() { String::new() } else { format!(" · {full_title}") },
                                                                tab_link(t).describe(),
                                                                t.panes.len()
                                                            ));
                                                        if r.clicked() {
                                                            *action = Some(Action::ActivateTab(i));
                                                        }
                                                        if r.clicked_by(egui::PointerButton::Middle)
                                                        {
                                                            *action = Some(Action::CloseTab(i));
                                                        }
                                                        r.context_menu(|ui| {
                                                            let others = self.tabs.len() > 1;
                                                            let disconnected =
                                                                self.tabs.iter().any(|tab| {
                                                                    tab_link(tab)
                                                                        == SessionStatus::Detached
                                                                });
                                                            for (text, enabled, a) in [
                                                                (
                                                                    "左右分屏",
                                                                    true,
                                                                    Action::Split(Axis::Horizontal),
                                                                ),
                                                                (
                                                                    "上下分屏",
                                                                    true,
                                                                    Action::Split(Axis::Vertical),
                                                                ),
                                                                (
                                                                    "复制会话",
                                                                    true,
                                                                    Action::DuplicateSession(i),
                                                                ),
                                                                (
                                                                    "复制选项卡信息",
                                                                    true,
                                                                    Action::CopyTabInfo(i),
                                                                ),
                                                                (
                                                                    "关闭标签",
                                                                    true,
                                                                    Action::CloseTab(i),
                                                                ),
                                                                (
                                                                    "关闭其它标签页",
                                                                    others,
                                                                    Action::CloseOtherTabs(i),
                                                                ),
                                                                (
                                                                    "关闭断开的标签页",
                                                                    disconnected,
                                                                    Action::CloseDisconnectedTabs,
                                                                ),
                                                            ] {
                                                                if ui
                                                                    .add_enabled(
                                                                        enabled,
                                                                        egui::Button::new(text),
                                                                    )
                                                                    .clicked()
                                                                {
                                                                    self.active = i;
                                                                    *action = Some(a);
                                                                    ui.close();
                                                                }
                                                            }
                                                        });
                                                    });
                                                if active {
                                                    let rect = tab_frame.response.rect;
                                                    let accent = connection_color(
                                                        &t.panes[t.focused].session.kind,
                                                        &self.settings.group_colors,
                                                    ).unwrap_or(p.accent);
                                                    ui.painter().rect_filled(
                                                        Rect::from_min_max(
                                                            egui::pos2(rect.left(), rect.bottom() - 2.5),
                                                            rect.right_bottom(),
                                                        ),
                                                        0,
                                                        accent,
                                                    );
                                                }
                                            });
                                        }
                                    });
                                });
                        });
                        if icons::icon_button(ui, icons::Icon::Plus, p, icons::Size::Button)
                            .on_hover_text("新建终端")
                            .clicked()
                        {
                            *action = Some(Action::New(SessionKind::Local(
                                self.settings.default_shell.clone(),
                            )));
                        }
                        let cursor = ui.cursor().min;
                        if cursor.x < buttons_left {
                            let drag_rect = Rect::from_min_max(
                                egui::Pos2::new(cursor.x, ui.max_rect().top()),
                                egui::Pos2::new(buttons_left, ui.max_rect().bottom()),
                            );
                            let drag = ui.interact(
                                drag_rect,
                                ui.id().with("titlebar-drag"),
                                Sense::click_and_drag(),
                            );
                            if drag.drag_started_by(egui::PointerButton::Primary) {
                                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                            }
                            if drag.double_clicked() {
                                toggle = true;
                            }
                        }
                    });
                });
                if toggle {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                }
                if close {
                    self.request_exit(ctx);
                }
            });
    }

    pub(super) fn update_window_title(&mut self, ctx: &egui::Context) {
        if self.screenshot.is_some() {
            return;
        }
        let shell = self.tabs.get(self.active).map(|tab| {
            let pane = &tab.panes[tab.focused];
            let title = pane
                .session
                .terminal
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .title()
                .to_string();
            if title.is_empty() {
                pane.session.kind.label()
            } else {
                title
            }
        });
        let title = match shell {
            Some(shell) if !shell.is_empty() => format!("{shell} 鈥?G-Terminal"),
            _ => "G-Terminal".to_string(),
        };
        if title != self.window_title {
            self.window_title = title.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
    }

    pub(super) fn mark_active_seen(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.seen_output = tab_output(tab);
            for pane in &tab.panes {
                pane.session
                    .traffic
                    .bell
                    .store(false, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum TitleButton {
    Close,
    Maximize,
    Restore,
    Minimize,
}

fn menu_heading(ui: &mut egui::Ui, title: &str, p: Palette) {
    ui.add_space(3.0);
    ui.label(egui::RichText::new(title).small().strong().color(p.muted));
    ui.add_space(2.0);
}

pub(super) fn brand_menu_button(
    ui: &mut egui::Ui,
    color: egui::Color32,
    capture_menu: bool,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let first = ui.fonts_mut(|fonts| {
        fonts.layout_no_wrap("G".to_owned(), egui::FontId::proportional(23.0), color)
    });
    let rest = ui.fonts_mut(|fonts| {
        fonts.layout_no_wrap("  菜单".to_owned(), egui::FontId::proportional(14.0), color)
    });

    let padding = ui.spacing().button_padding;
    let size = egui::vec2(
        first.size().x + rest.size().x + padding.x * 2.0,
        (first.size().y.max(rest.size().y) + padding.y * 2.0).max(ui.spacing().interact_size.y),
    );
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, false);
        if response.hovered() || response.is_pointer_button_down_on() {
            ui.painter().rect(
                rect,
                visuals.corner_radius,
                visuals.weak_bg_fill,
                visuals.bg_stroke,
                egui::StrokeKind::Inside,
            );
        }

        let ink_centre = |galley: &egui::Galley| {
            galley.rows.first().map_or(galley.size().y / 2.0, |row| {
                row.visuals.mesh_bounds.center().y
            })
        };
        let middle = rect.center().y;
        let first_pos = egui::pos2(rect.left() + padding.x, middle - ink_centre(&first));
        let rest_pos = egui::pos2(first_pos.x + first.size().x, middle - ink_centre(&rest));
        let painter = ui.painter();
        painter.galley(first_pos, first, color);
        painter.galley(rest_pos, rest, color);
    }
    let popup = egui::Popup::menu(&response);
    let popup = if capture_menu {
        popup.open(true)
    } else {
        popup
    };
    popup.show(|ui| add_contents(ui));
    response
}

pub(super) fn topbar_margin() -> egui::Margin {
    if cfg!(target_os = "macos") {
        egui::Margin {
            left: 78,
            right: 4,
            top: 2,
            bottom: 2,
        }
    } else {
        egui::Margin::symmetric(10, 4)
    }
}

pub(super) fn title_button(ui: &mut egui::Ui, kind: TitleButton, p: Palette) -> egui::Response {
    let icon = match kind {
        TitleButton::Close => icons::Icon::Close,
        TitleButton::Maximize => icons::Icon::Maximize,
        TitleButton::Restore => icons::Icon::Restore,
        TitleButton::Minimize => icons::Icon::Minimize,
    };
    icons::window_button(ui, icon, p, kind == TitleButton::Close)
}
