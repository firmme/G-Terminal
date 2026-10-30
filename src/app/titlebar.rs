//! The titlebar: the frameless window's chrome, the brand menu and the tab strip.

use super::*;
use g_terminal::LockExt;

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
                        brand_menu_button(ui, p, capture_menu, |ui| {
                            ui.set_width(292.0);
                            // New sessions: the most used entries, as tiles.
                            let local = Action::New(SessionKind::Local(
                                self.settings.default_shell.clone(),
                            ));
                            let mut picked = None;
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                for (icon, caption, tip, a) in [
                                    (
                                        icons::Icon::Terminal,
                                        "终端",
                                        format!("新建终端 ({})", accel("Ctrl+Shift+T")),
                                        local,
                                    ),
                                    (
                                        icons::Icon::Host,
                                        "SSH",
                                        "新建 SSH 连接".to_string(),
                                        Action::Remote,
                                    ),
                                    (
                                        icons::Icon::Serial,
                                        "串口",
                                        "连接串口".to_string(),
                                        Action::SerialPicker,
                                    ),
                                    (
                                        icons::Icon::NewWindow,
                                        "窗口",
                                        "新建窗口".to_string(),
                                        Action::NewWindow,
                                    ),
                                ] {
                                    if icons::tile(ui, icon, caption, p).on_hover_text(tip).clicked() {
                                        picked = Some(a);
                                    }
                                }
                            });
                            menu_heading(ui, "当前会话", p);
                            let live = self.active_link() == Some(SessionStatus::Live);
                            if live
                                && icons::icon_row(
                                    ui,
                                    icons::Icon::Disconnect,
                                    "断开",
                                    Some(alt_accel("C").as_str()),
                                    p,
                                )
                                .clicked()
                            {
                                picked = Some(Action::Disconnect);
                            }
                            if (!live || !self.active_is_remote())
                                && icons::icon_row(
                                    ui,
                                    icons::Icon::Reconnect,
                                    if live { "重启" } else { "重新连接" },
                                    Some(alt_accel("R").as_str()),
                                    p,
                                )
                                .clicked()
                            {
                                picked = Some(Action::Restart);
                            }
                            if icons::icon_row(
                                ui,
                                icons::Icon::Search,
                                "查找输出",
                                Some(accel("Ctrl+Shift+F").as_str()),
                                p,
                            )
                            .clicked()
                            {
                                self.search_open = true;
                                self.search_focus = true;
                                ui.close();
                            }
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                for (icon, caption, tip, a) in [
                                    (
                                        icons::Icon::SplitHorizontal,
                                        "左右",
                                        format!("左右分屏 ({})", accel("Ctrl+Shift+D")),
                                        Action::Split(Axis::Horizontal),
                                    ),
                                    (
                                        icons::Icon::SplitVertical,
                                        "上下",
                                        format!("上下分屏 ({})", accel("Ctrl+Shift+E")),
                                        Action::Split(Axis::Vertical),
                                    ),
                                    (
                                        icons::Icon::ClosePane,
                                        "关闭",
                                        format!("关闭窗格 ({})", accel("Ctrl+Shift+W")),
                                        Action::ClosePane,
                                    ),
                                ] {
                                    if icons::tile(ui, icon, caption, p).on_hover_text(tip).clicked() {
                                        picked = Some(a);
                                    }
                                }
                            });
                            menu_heading(ui, "工具", p);
                            if icons::icon_row(ui, icons::Icon::Folder, "文件与传输", None, p)
                                .clicked()
                            {
                                picked = Some(Action::Files);
                            }
                            if icons::icon_row(ui, icons::Icon::Group, "分组管理", None, p)
                                .clicked()
                            {
                                self.groups_open = true;
                                ui.close();
                            }
                            if icons::icon_row(ui, icons::Icon::Serial, "串口占用排查", None, p)
                                .clicked()
                            {
                                picked = Some(Action::FindPortOwner(self.port_owner_default()));
                            }
                            if icons::icon_row(ui, icons::Icon::Toolbox, "服务器工具箱", None, p)
                                .clicked()
                            {
                                picked = Some(Action::Toolbox);
                            }
                            ui.add_space(4.0);
                            ui.separator();
                            // App-level controls as a quiet icon bar.
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 2.0;
                                if icons::icon_toggle(
                                    ui,
                                    icons::Icon::Sidebar,
                                    self.settings.sidebar,
                                    p,
                                )
                                .on_hover_text("显示导航栏")
                                .clicked()
                                {
                                    self.settings.sidebar = !self.settings.sidebar;
                                    ui.close();
                                }
                                for (icon, tip, target) in [
                                    (
                                        icons::Icon::Settings,
                                        format!("偏好设置 ({})", accel("Ctrl+,")),
                                        0,
                                    ),
                                    (icons::Icon::Keyboard, "快捷键与关于".to_string(), 1),
                                    (icons::Icon::Refresh, "检查更新".to_string(), 2),
                                ] {
                                    if icons::icon_button(ui, icon, p, icons::Size::Button)
                                        .on_hover_text(tip)
                                        .clicked()
                                    {
                                        match target {
                                            0 => self.settings_open = true,
                                            1 => self.help_open = true,
                                            _ => picked = Some(Action::CheckUpdates),
                                        }
                                        ui.close();
                                    }
                                }
                                let github = icons::icon_button(
                                    ui, icons::Icon::GitHub, p, icons::Size::Button,
                                ).on_hover_text("GitHub 项目主页");
                                github.widget_info(|| egui::WidgetInfo::labeled(
                                    egui::WidgetType::Button, ui.is_enabled(), "GitHub 项目主页",
                                ));
                                if github.clicked() {
                                    ui.ctx().open_url(egui::OpenUrl::new_tab(
                                        "https://github.com/firmme/G-Terminal",
                                    ));
                                    ui.close();
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if icons::icon_button(ui, icons::Icon::Power, p, icons::Size::Button)
                                        .on_hover_text("退出")
                                        .clicked()
                                    {
                                        picked = Some(Action::Exit);
                                    }
                                    ui.label(hint(concat!("v", env!("CARGO_PKG_VERSION")), p));
                                });
                            });
                            if let Some(a) = picked {
                                *action = Some(a);
                                ui.close();
                            }
                        });
                        let auto_hide = self.settings.auto_hide_sidebar;
                        let docked = self.settings.sidebar && (!auto_hide || self.sidebar_pinned);
                        let hover = if auto_hide {
                            if docked {
                                "改为自动隐藏"
                            } else {
                                "固定展开导航栏"
                            }
                        } else if self.settings.sidebar {
                            "收起导航栏"
                        } else {
                            "展开导航栏"
                        };
                        if icons::icon_toggle(ui, icons::Icon::Sidebar, docked, p)
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
                                                                icons::draw(ui.painter(), dot.shrink(1.0), icons::Icon::Reconnect, p.accent, 1.4);
                                                            } else {
                                                                // What the tab is, with how it is doing as a badge.
                                                                icons::draw(
                                                                    ui.painter(),
                                                                    dot.shrink(1.0),
                                                                    sidebar::kind_icon(&t.panes[t.focused].session.kind),
                                                                    if active { p.text } else { p.muted },
                                                                    1.3,
                                                                );
                                                                let badge = dot.right_bottom() - egui::vec2(2.5, 2.5);
                                                                ui.painter().circle_filled(badge, 4.0, fill);
                                                                ui.painter().circle_filled(
                                                                    badge,
                                                                    2.8,
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
                                                                        RichText::new("·")
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
                                                                    status::connecting_spinner(ui, p);
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
                                                            let close = icons::close_button(ui, p, "关闭标签");
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
                                                                .locked()
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
                                                            for (icon, text, enabled, a) in [
                                                                (
                                                                    icons::Icon::SplitHorizontal,
                                                                    "左右分屏",
                                                                    true,
                                                                    Action::Split(Axis::Horizontal),
                                                                ),
                                                                (
                                                                    icons::Icon::SplitVertical,
                                                                    "上下分屏",
                                                                    true,
                                                                    Action::Split(Axis::Vertical),
                                                                ),
                                                                (
                                                                    icons::Icon::Plus,
                                                                    "复制会话",
                                                                    true,
                                                                    Action::DuplicateSession(i),
                                                                ),
                                                                (
                                                                    icons::Icon::Copy,
                                                                    "复制标签信息",
                                                                    true,
                                                                    Action::CopyTabInfo(i),
                                                                ),
                                                                (
                                                                    icons::Icon::Close,
                                                                    "关闭",
                                                                    true,
                                                                    Action::CloseTab(i),
                                                                ),
                                                                (
                                                                    icons::Icon::Close,
                                                                    "关闭其它",
                                                                    others,
                                                                    Action::CloseOtherTabs(i),
                                                                ),
                                                                (
                                                                    icons::Icon::Disconnect,
                                                                    "关闭已断开",
                                                                    disconnected,
                                                                    Action::CloseDisconnectedTabs,
                                                                ),
                                                            ] {
                                                                if ui
                                                                    .add_enabled_ui(enabled, |ui| {
                                                                        icons::icon_row(ui, icon, text, None, p)
                                                                    })
                                                                    .inner
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
            let title = pane.session.terminal.locked().title().to_string();
            if title.is_empty() {
                pane.session.kind.label()
            } else {
                title
            }
        });
        let title = match shell {
            Some(shell) if !shell.is_empty() => format!("{shell} — G-Terminal"),
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
    ui.add_space(8.0);
    ui.label(egui::RichText::new(title).small().strong().color(p.muted));
    ui.add_space(2.0);
}

pub(super) fn brand_menu_button(
    ui: &mut egui::Ui,
    p: Palette,
    capture_menu: bool,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let mark = ui.fonts_mut(|fonts| {
        fonts.layout_no_wrap("G".to_owned(), egui::FontId::proportional(22.0), p.accent)
    });
    let size = egui::vec2(mark.size().x + 30.0, 30.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let response = response.on_hover_text("菜单");
    if ui.is_rect_visible(rect) {
        let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
        if response.hovered() || open {
            ui.painter()
                .rect_filled(rect, 7, blend(p.panel, p.raised, 0.8));
        }
        let ink = mark.rows.first().map_or(mark.size().y / 2.0, |row| {
            row.visuals.mesh_bounds.center().y
        });
        let mark_pos = egui::pos2(rect.left() + 9.0, rect.center().y - ink);
        let chevron = Rect::from_center_size(
            egui::pos2(rect.right() - 10.0, rect.center().y + 1.0),
            egui::Vec2::splat(10.0),
        );
        let width = mark.size().x;
        ui.painter().galley(mark_pos, mark, p.accent);
        icons::draw(
            ui.painter(),
            chevron,
            icons::Icon::ChevronDown,
            if response.hovered() { p.text } else { p.muted },
            1.5,
        );
        let _ = width;
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
