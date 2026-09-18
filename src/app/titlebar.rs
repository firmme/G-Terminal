//! The titlebar: the frameless window's chrome, the brand menu and the tab strip.

use super::*;

impl App {
    /// Claims the window's outer few pixels for resizing.
    ///
    /// A frameless window gets no resize border from the OS, and winit does not add
    /// one — so without this the window simply cannot be resized by dragging, which
    /// is what it did before. The edges are detected here and handed to the
    /// platform, which then runs its own resize loop.
    pub(super) fn resize_grips(&self, ctx: &egui::Context) {
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
        // The cursor is the whole affordance: nothing is drawn, so without it the
        // edge looks inert.
        ctx.set_cursor_icon(match direction {
            ResizeDirection::North | ResizeDirection::South => egui::CursorIcon::ResizeVertical,
            ResizeDirection::East | ResizeDirection::West => egui::CursorIcon::ResizeHorizontal,
            ResizeDirection::NorthWest | ResizeDirection::SouthEast => egui::CursorIcon::ResizeNwSe,
            ResizeDirection::NorthEast | ResizeDirection::SouthWest => egui::CursorIcon::ResizeNeSw,
        });
        // Only a press that starts on the edge begins a resize; otherwise every
        // click near a border would jump into one.
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
                // Read maximized fresh every frame: the window can also be maximized
                // by the OS (snap, Win+Up, taskbar) without going through a button.
                let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                // The Windows and Linux windows are frameless, so they draw their
                // own buttons and resize grips. macOS hides the native titlebar
                // but keeps its buttons and edge resizing, so it draws neither.
                let custom_chrome = !cfg!(target_os = "macos");
                // The tab row and the window buttons share one bar, so the frameless
                // chrome costs a single row of height instead of two.
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
                        // Derive the edge from the buttons themselves; `min_rect()` would
                        // also fold in unrelated widgets. The drag strip below is
                        // registered later and would win any overlap, so this boundary
                        // has to be exact.
                        close_button
                            .rect
                            .union(toggle_button.rect)
                            .union(minimize_button.rect)
                            .left()
                            - 4.0
                    } else {
                        // macOS: no custom buttons, but the drag strip still runs
                        // to the right edge of the bar.
                        ui.max_rect().right()
                    };
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        brand_menu_button(ui, p.accent, |ui| {
                            for (icon, text, shortcut, a) in [
                                (
                                    icons::Icon::Terminal,
                                    "新建终端",
                                    Some(accel("Ctrl+Shift+T")),
                                    Action::New(SessionKind::Local(
                                        self.settings.default_shell.clone(),
                                    )),
                                ),
                                (icons::Icon::Terminal, "新建窗口", None, Action::NewWindow),
                                (icons::Icon::Host, "新建 SSH 连接", None, Action::Remote),
                                (
                                    icons::Icon::Terminal,
                                    "连接串口",
                                    None,
                                    Action::SerialPicker,
                                ),
                                (
                                    icons::Icon::Host,
                                    "串口占用排查",
                                    None,
                                    Action::FindPortOwner(self.port_owner_default()),
                                ),
                                (icons::Icon::File, "文件与传输队列", None, Action::Files),
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
                                if icons::icon_row(ui, icon, text, shortcut.as_deref(), p).clicked()
                                {
                                    *action = Some(a);
                                    ui.close();
                                }
                            }
                            // Only what the current link state can do is shown:
                            // a connected session can be disconnected, a
                            // dropped one reconnected. A live *local* shell can
                            // still be restarted, so it keeps that entry.
                            let live = self.active_link() == Some(SessionStatus::Live);
                            if live
                                && icons::icon_row(
                                    ui,
                                    icons::Icon::ClosePane,
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
                                    icons::Icon::Restart,
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
                            ui.separator();
                            if ui
                                .checkbox(&mut self.settings.sidebar, "显示导航栏")
                                .changed()
                            {
                                ui.close();
                            }
                            if icons::icon_row(ui, icons::Icon::Group, "连接分组管理", None, p)
                                .clicked()
                            {
                                self.groups_open = true;
                                ui.close();
                            }
                            if icons::icon_row(ui, icons::Icon::Settings, "服务器工具箱", None, p)
                                .clicked()
                            {
                                *action = Some(Action::Toolbox);
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
                            ui.separator();
                            ui.label(hint(
                                concat!(
                                    "G-Terminal ",
                                    env!("CARGO_PKG_VERSION"),
                                    " · Native. Fast. Yours."
                                ),
                                p,
                            ));
                        });
                        // In auto-hide mode the button no longer hides the bar
                        // outright: expanding pins it open, and hiding sends it
                        // back to hovering. Outside that mode it stays a plain
                        // show/hide toggle.
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
                        // Bound the strip so a stretch of empty bar always remains for
                        // dragging the window by.
                        const DRAG_GAP: f32 = 60.0;
                        let strip_max = (buttons_left - ui.cursor().min.x - DRAG_GAP).max(120.0);
                        // A horizontal strip ignores the wheel, and every mouse has
                        // one. The vertical delta is fed in as horizontal, but only
                        // while the pointer is over the strip — otherwise the
                        // terminal below would stop scrolling.
                        let cursor = ui.cursor().min;
                        let strip_rect = egui::Rect::from_min_max(
                            cursor,
                            egui::pos2(cursor.x + strip_max, cursor.y + 24.0),
                        );
                        if ui.rect_contains_pointer(strip_rect) {
                            ui.ctx().input_mut(|input| {
                                let wheel = input.smooth_scroll_delta.y;
                                input.smooth_scroll_delta.x += wheel;
                                input.smooth_scroll_delta.y = 0.0;
                            });
                        }
                        // A hairline bar, shown only when the tabs actually
                        // overflow: it reads as an indicator, not as chrome.
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
                                            // A background tab that has received
                                            // more than it had when it was last
                                            // looked at is underlined.
                                            let updated = tab_updated(t, i == self.active);
                                            ui.push_id(t.id, |ui| {
                                                // The hover state comes from the
                                                // previous frame's response, which
                                                // is what lets the background be
                                                // painted before the labels; the
                                                // tab is interactive, so moving
                                                // onto it already schedules the
                                                // repaint that shows the highlight.
                                                let hit_id = ui.id().with("tab-hit");
                                                let hovered = ui
                                                    .ctx()
                                                    .read_response(hit_id)
                                                    .is_some_and(|r| r.hovered());
                                                // A connection or group colour
                                                // tints the whole label; the
                                                // connection's own colour wins.
                                                let active = i == self.active;
                                                let base = if active { p.raised } else { p.panel };
                                                // Just enough tint to tell the three states
                                                // apart: the active tab is a touch brighter with
                                                // a soft outline, hover a touch brighter still
                                                // than the plain panel — no shouting.
                                                let (fill, stroke) = match connection_color(
                                                    &t.panes[t.focused].session.kind,
                                                    &self.settings.group_colors,
                                                ) {
                                                    Some(color) => (
                                                        blend(
                                                            base,
                                                            color,
                                                            if active {
                                                                0.35
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
                                                            egui::Stroke::NONE
                                                        },
                                                    ),
                                                    None if active => (
                                                        blend(p.raised, p.accent, 0.16),
                                                        egui::Stroke::new(
                                                            1.0_f32,
                                                            p.accent.gamma_multiply(0.45),
                                                        ),
                                                    ),
                                                    None if hovered => (
                                                        blend(p.panel, p.accent, 0.10),
                                                        egui::Stroke::NONE,
                                                    ),
                                                    None => (p.panel, egui::Stroke::NONE),
                                                };
                                                egui::Frame::new()
                                                    .fill(fill)
                                                    .stroke(stroke)
                                                    .inner_margin(egui::Margin::symmetric(6, 0))
                                                    .show(ui, |ui| {
                                                        let mut hit = None;
                                                        ui.horizontal(|ui| {
                                                            // Connection state of the whole tab, worst pane wins.
                                                            let (dot, _) = ui.allocate_exact_size(
                                                                egui::vec2(7.0, 7.0),
                                                                Sense::hover(),
                                                            );
                                                            ui.painter().circle_filled(
                                                                dot.center(),
                                                                3.5,
                                                                link_color(tab_link(t), p),
                                                            );
                                                            ui.interact(
                                                                dot,
                                                                ui.id().with("tab-state"),
                                                                Sense::hover(),
                                                            )
                                                            .on_hover_text(tab_link(t).describe());
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
                                                                // The shell's own title (OSC 0/2) wins over the
                                                                // session label once it has set one.
                                                                let title = pane
                                                                    .session
                                                                    .terminal
                                                                    .lock()
                                                                    .unwrap_or_else(|e| e.into_inner())
                                                                    .title()
                                                                    .to_string();
                                                                let text = if title.is_empty() {
                                                                    pane.session.kind.label()
                                                                } else {
                                                                    title
                                                                }
                                                                .chars()
                                                                .take(18)
                                                                .collect::<String>();
                                                                let label = RichText::new(text)
                                                                    .color(color);
                                                                let label = if focused {
                                                                    label.strong()
                                                                } else {
                                                                    label
                                                                };
                                                                // Underlined while the
                                                                // tab holds output the
                                                                // user has not seen.
                                                                ui.label(if updated {
                                                                    label.underline()
                                                                } else {
                                                                    label
                                                                });
                                                            }
                                                            // A background tab whose session rang shows a
                                                            // bell until the tab is looked at.
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
                                                            // Inside the label row, so it hugs the
                                                            // text instead of floating at the tab's
                                                            // far edge.
                                                            let close = icons::glyph_button(
                                                                ui,
                                                                "×",
                                                                p,
                                                                "关闭标签",
                                                            );
                                                            if close.clicked() {
                                                                *action = Some(Action::CloseTab(i));
                                                            }
                                                            // The whole tab activates on click, but the
                                                            // hit rect has to stop where the close button
                                                            // begins — it is registered first, so anything
                                                            // overlapping it would win.
                                                            let label_rect = ui.min_rect();
                                                            hit = Some(egui::Rect::from_min_max(
                                                                label_rect.min,
                                                                egui::pos2(
                                                                    close.rect.left(),
                                                                    label_rect.max.y,
                                                                ),
                                                            ));
                                                        });
                                                        let Some(rect) = hit else { return };
                                                        let r = ui
                                                            .interact(rect, hit_id, Sense::click())
                                                            .on_hover_text(format!(
                                                                "{} · {} 个窗格 · 中键关闭",
                                                                t.panes[t.focused]
                                                                    .session
                                                                    .kind
                                                                    .label(),
                                                                t.panes.len()
                                                            ));
                                                        if r.clicked() {
                                                            self.active = i;
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
                        // Whatever the tabs left over, up to the buttons, drags the
                        // frameless window. Spanning the full bar height keeps the
                        // top and bottom rows of pixels draggable too.
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

    /// Keeps the OS window title on the focused shell's own title (OSC 0/2),
    /// falling back to its session label. Sent only when it changes.
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

    /// Looking at a tab clears its new-output mark and its bell; the tab strip
    /// has already been drawn this frame.
    pub(super) fn mark_active_seen(&mut self) {
        // Looking at a tab clears its new-output mark, which is why this lands
        // after the strip has been drawn.
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.seen_output = tab_output(tab);
            // The active tab's bell has been seen; clear it.
            for pane in &tab.panes {
                pane.session
                    .traffic
                    .bell
                    .store(false, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

/// Which frame icon a titlebar button paints. `Restore` and `Maximize` are the
/// same button in two states.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum TitleButton {
    Close,
    Maximize,
    Restore,
    Minimize,
}

/// The menu button. `G` is a Latin capital, which is shorter than the CJK glyphs
/// beside it, so it is drawn one size up and the two runs are lined up by their
/// painted bounds instead of egui's galley centring, which leaves `G` small and
/// low against 菜单.
pub(super) fn brand_menu_button(
    ui: &mut egui::Ui,
    color: egui::Color32,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let first = ui.fonts_mut(|fonts| {
        fonts.layout_no_wrap("G".to_owned(), egui::FontId::proportional(17.0), color)
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
        ui.painter().rect(
            rect,
            visuals.corner_radius,
            visuals.weak_bg_fill,
            visuals.bg_stroke,
            egui::StrokeKind::Inside,
        );

        // Painted bounds, not font metrics: the glyphs' visual centres are what
        // has to agree, and a CJK run's metrics say little about where its ink
        // sits. `mesh_bounds` is each run's ink.
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
    egui::Popup::menu(&response).show(|ui| add_contents(ui));
    response
}

/// The topbar's inner margin. macOS runs the content under a hidden native
/// titlebar, so the left side has to clear the traffic lights; every other
/// platform owns the whole row.
pub(super) fn topbar_margin() -> egui::Margin {
    if cfg!(target_os = "macos") {
        egui::Margin {
            left: 78,
            right: 4,
            top: 2,
            bottom: 2,
        }
    } else {
        egui::Margin::symmetric(4, 2)
    }
}

/// The window-chrome buttons. Painted through the shared icon set, for the same
/// reason every other glyph is: no font in the loaded chain carries these
/// codepoints, so the text versions rendered as tofu boxes.
pub(super) fn title_button(ui: &mut egui::Ui, kind: TitleButton, p: Palette) -> egui::Response {
    let icon = match kind {
        TitleButton::Close => icons::Icon::Close,
        TitleButton::Maximize => icons::Icon::Maximize,
        TitleButton::Restore => icons::Icon::Restore,
        TitleButton::Minimize => icons::Icon::Minimize,
    };
    icons::window_button(ui, icon, p, kind == TitleButton::Close)
}
