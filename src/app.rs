use crate::{
    icons,
    remote_ui::{self, Files, Login, format_size, hint},
    theme::{Palette, load_fonts},
    view::Pane,
};
use eframe::egui::{self, Align, Key, Layout, Rect, RichText, Sense};
use g_terminal::{
    config::{Forward, RemoteProfile, Settings},
    layout::{Axis, Layout as PaneLayout, SavedTab},
    remote::Connection,
    session::{Session, SessionKind, SessionStatus},
};
use std::sync::Arc;

struct Tab {
    id: u64,
    panes: Vec<Pane>,
    focused: usize,
    layout: PaneLayout,
}
#[derive(Clone)]
enum Action {
    New(SessionKind),
    Split(Axis),
    CloseTab(usize),
    ClosePane,
    Restart,
    Remote,
    Edit(usize),
    Remove(usize),
    Files,
}
pub struct App {
    settings: Settings,
    palette: Palette,
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    settings_open: bool,
    remote_open: bool,
    help_open: bool,
    groups_open: bool,
    remote: RemoteProfile,
    editing_profile: Option<usize>,
    new_group: String,
    search_open: bool,
    search: String,
    search_focus: bool,
    search_hits: Vec<(usize, u16)>,
    search_index: usize,
    error: Option<String>,
    login: Option<Login>,
    login_target: Option<(u64, u64)>,
    files: Option<Files>,
    files_open: bool,
    split_chooser: Option<(u64, u64)>,
    screenshot: Option<std::path::PathBuf>,
    started: std::time::Instant,
    screenshot_requested: bool,
}
impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, screenshot: Option<std::path::PathBuf>) -> Self {
        Self::new_context(&cc.egui_ctx, screenshot)
    }
    pub(crate) fn new_context(ctx: &egui::Context, screenshot: Option<std::path::PathBuf>) -> Self {
        let (settings, error) = match Settings::load() {
            Ok(s) => (s, None),
            Err(e) => (Settings::default(), Some(e.to_string())),
        };
        Self::from_settings(ctx, settings, error, screenshot)
    }
    fn from_settings(
        ctx: &egui::Context,
        settings: Settings,
        error: Option<String>,
        screenshot: Option<std::path::PathBuf>,
    ) -> Self {
        let palette = Palette::new(settings.light_theme);
        load_fonts(ctx);
        palette.apply(ctx, settings.light_theme);
        let mut app = Self {
            settings,
            palette,
            tabs: vec![],
            active: 0,
            next_id: 0,
            settings_open: false,
            remote_open: false,
            help_open: false,
            groups_open: false,
            remote: RemoteProfile::default(),
            editing_profile: None,
            new_group: String::new(),
            search_open: false,
            search: String::new(),
            search_focus: false,
            search_hits: vec![],
            search_index: 0,
            error,
            login: None,
            login_target: None,
            files: None,
            files_open: false,
            split_chooser: None,
            screenshot,
            started: std::time::Instant::now(),
            screenshot_requested: false,
        };
        if app.settings.restore_workspace && app.screenshot.is_none() {
            app.restore(ctx);
        }
        if app.tabs.is_empty() {
            app.execute(
                Action::New(SessionKind::Local(app.settings.default_shell.clone())),
                ctx,
            );
        }
        app
    }
    fn spawn(&mut self, kind: SessionKind, ctx: &egui::Context) -> Option<Pane> {
        match Session::spawn(kind, self.settings.scrollback, remote_ui::wake(ctx)) {
            Ok(s) => {
                self.next_id += 1;
                Some(Pane::new(self.next_id, s))
            }
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                None
            }
        }
    }
    fn add_connection(&mut self, connection: Arc<Connection>, ctx: &egui::Context) {
        self.next_id += 1;
        let pane = Pane::new(
            self.next_id,
            Session::from_remote(connection, self.settings.scrollback, remote_ui::wake(ctx)),
        );
        if let Some((tab_id, pane_id)) = self.login_target.take()
            && let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id)
            && let Some(index) = t.panes.iter().position(|p| p.id == pane_id)
        {
            t.panes[index] = pane;
            t.focused = index;
            return;
        }
        self.tabs.push(Tab {
            id: pane.id,
            panes: vec![pane],
            focused: 0,
            layout: PaneLayout::Leaf(0),
        });
        self.active = self.tabs.len() - 1;
    }
    fn snapshot(&mut self) {
        self.settings.workspace = self
            .tabs
            .iter()
            .map(|t| SavedTab {
                sessions: t.panes.iter().map(|p| p.session.kind.clone()).collect(),
                layout: t.layout.clone(),
                focused: t.focused,
            })
            .collect();
    }
    fn persist(&mut self) {
        self.snapshot();
        if let Err(e) = self.settings.save() {
            self.error = Some(format!("{e:#}"));
        }
    }
    fn restore(&mut self, ctx: &egui::Context) {
        for saved in self.settings.workspace.clone().into_iter().take(32) {
            if saved.sessions.is_empty()
                || saved.sessions.len() > 32
                || !saved.layout.valid(saved.sessions.len())
            {
                continue;
            }
            let mut panes = vec![];
            for kind in saved.sessions {
                if matches!(kind, SessionKind::Local(_)) {
                    if let Some(p) = self.spawn(kind, ctx) {
                        panes.push(p);
                    } else {
                        break;
                    }
                } else {
                    self.next_id += 1;
                    panes.push(Pane::new(
                        self.next_id,
                        Session::disconnected(kind, self.settings.scrollback),
                    ));
                }
            }
            if !panes.is_empty() && saved.layout.valid(panes.len()) {
                self.tabs.push(Tab {
                    id: panes[0].id,
                    focused: saved.focused.min(panes.len() - 1),
                    panes,
                    layout: saved.layout,
                });
            }
        }
    }
    fn execute(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::New(SessionKind::Ssh(p)) | Action::New(SessionKind::Sftp(p)) => {
                self.login_target = None;
                self.login = Some(Login::new(p))
            }
            Action::New(kind) => {
                if let Some(pane) = self.spawn(kind, ctx) {
                    self.tabs.push(Tab {
                        id: pane.id,
                        panes: vec![pane],
                        focused: 0,
                        layout: PaneLayout::Leaf(0),
                    });
                    self.active = self.tabs.len() - 1;
                }
            }
            Action::Split(axis) => {
                if self
                    .tabs
                    .get(self.active)
                    .is_some_and(|t| t.panes.len() < 32)
                {
                    let connection = self.tabs[self.active].panes[self.tabs[self.active].focused]
                        .session
                        .remote
                        .clone();
                    let pane = if let Some(c) = connection {
                        self.next_id += 1;
                        Some(Pane::new(
                            self.next_id,
                            Session::from_remote(c, self.settings.scrollback, remote_ui::wake(ctx)),
                        ))
                    } else {
                        self.spawn(SessionKind::Local(self.settings.default_shell.clone()), ctx)
                    };
                    if let Some(p) = pane {
                        let pane_id = p.id;
                        let tab_id = self.tabs[self.active].id;
                        let t = &mut self.tabs[self.active];
                        t.layout.split(t.focused, t.panes.len(), axis);
                        t.focused = t.panes.len();
                        t.panes.push(p);
                        self.split_chooser = Some((tab_id, pane_id));
                    }
                }
            }
            Action::CloseTab(i) => {
                if i < self.tabs.len() {
                    self.tabs.remove(i);
                    if self.active > i {
                        self.active -= 1;
                    }
                    self.active = self.active.min(self.tabs.len().saturating_sub(1));
                }
            }
            Action::ClosePane => {
                if let Some(t) = self.tabs.get_mut(self.active) {
                    if t.panes.len() > 1 {
                        t.layout = t.layout.clone().remove(t.focused).unwrap();
                        t.panes.remove(t.focused);
                        t.focused = t.focused.min(t.panes.len() - 1);
                    } else {
                        self.execute(Action::CloseTab(self.active), ctx);
                    }
                }
            }
            Action::Restart => {
                if let Some(t) = self.tabs.get(self.active) {
                    let kind = t.panes[t.focused].session.kind.clone();
                    if let SessionKind::Ssh(p) | SessionKind::Sftp(p) = kind {
                        self.login_target = Some((t.id, t.panes[t.focused].id));
                        self.login = Some(Login::new(p));
                    } else if let Some(p) = self.spawn(kind, ctx) {
                        let t = &mut self.tabs[self.active];
                        t.panes[t.focused] = p;
                    }
                }
            }
            Action::Remote => {
                self.remote = RemoteProfile::default();
                self.editing_profile = None;
                self.remote_open = true;
            }
            Action::Edit(i) => {
                self.remote = self.settings.profiles[i].clone();
                self.editing_profile = Some(i);
                self.remote_open = true;
            }
            Action::Remove(i) => {
                self.settings.profiles.remove(i);
                self.persist();
            }
            Action::Files => {
                if let Some(c) = self
                    .tabs
                    .get(self.active)
                    .and_then(|t| t.panes[t.focused].session.remote.clone())
                {
                    if self
                        .files
                        .as_ref()
                        .is_none_or(|f| !Arc::ptr_eq(&f.connection, &c))
                    {
                        if self.files.as_ref().is_some_and(|f| {
                            f.transfers.iter().any(|t| t.state.lock().unwrap().running)
                        }) {
                            self.error =
                                Some("文件窗口还有传输任务，请完成或暂停后切换连接".into());
                            return;
                        }
                        self.files = Some(Files::new(c, ctx, self.settings.hide_dotfiles));
                    }
                    self.files_open = true;
                } else {
                    self.error = Some("请先连接内置 SSH 会话".into());
                }
            }
        }
        self.search_hits.clear();
    }
    fn shortcuts(&mut self, ctx: &egui::Context) -> Option<Action> {
        if self.settings_open
            || self.remote_open
            || self.groups_open
            || self.help_open
            || self.login.is_some()
            || self.files_open
        {
            return None;
        }
        let mut action = None;
        ctx.input_mut(|i| {
            i.events.retain(|e| {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = e
                {
                    if modifiers.ctrl && modifiers.shift {
                        match key {
                            Key::T => {
                                action = Some(Action::New(SessionKind::Local(
                                    self.settings.default_shell.clone(),
                                )))
                            }
                            Key::W => action = Some(Action::ClosePane),
                            Key::D => action = Some(Action::Split(Axis::Horizontal)),
                            Key::E => action = Some(Action::Split(Axis::Vertical)),
                            Key::F => {
                                self.search_open = !self.search_open;
                                self.search_focus = self.search_open;
                            }
                            Key::B => self.settings.sidebar = !self.settings.sidebar,
                            _ => return true,
                        }
                        return false;
                    }
                    if modifiers.ctrl && *key == Key::Tab {
                        if !self.tabs.is_empty() {
                            self.active = (self.active + 1) % self.tabs.len();
                        }
                        return false;
                    }
                    if modifiers.ctrl && *key == Key::Comma {
                        self.settings_open = true;
                        return false;
                    }
                    if modifiers.alt && *key == Key::ArrowRight {
                        if let Some(t) = self.tabs.get_mut(self.active) {
                            t.focused = (t.focused + 1) % t.panes.len();
                        }
                        return false;
                    }
                    if modifiers.ctrl
                        && matches!(key, Key::Plus | Key::Equals | Key::Minus | Key::Num0)
                    {
                        self.settings.font_size = match key {
                            Key::Minus => self.settings.font_size - 1.0,
                            Key::Num0 => 15.0,
                            _ => self.settings.font_size + 1.0,
                        }
                        .clamp(10.0, 28.0);
                        return false;
                    }
                    if *key == Key::Escape && self.search_open {
                        self.search_open = false;
                        return false;
                    }
                }
                true
            })
        });
        action
    }
    fn begin_terminal_zmodem(
        &mut self,
        upload: bool,
        local: std::path::PathBuf,
        connection: &Arc<Connection>,
        ctx: &egui::Context,
    ) {
        if let Some(files) = &self.files
            && !Arc::ptr_eq(&files.connection, connection)
            && files
                .transfers
                .iter()
                .any(|t| t.state.lock().unwrap().running)
        {
            self.error = Some("文件窗口还有其他连接的传输任务，请完成后再试".into());
            return;
        }
        if self.files.as_ref().is_some_and(Files::busy) {
            self.error = Some("文件窗口还有 ZMODEM 传输任务，请完成或取消后再试".into());
            return;
        }
        if self
            .files
            .as_ref()
            .is_none_or(|f| !Arc::ptr_eq(&f.connection, connection))
        {
            self.files = Some(Files::new(
                connection.clone(),
                ctx,
                self.settings.hide_dotfiles,
            ));
        }
        // The file window is not opened. A transfer started from the terminal has
        // nothing to do with the file browser, and the status bar already carries
        // the progress strip for it.
        let grab = self.tabs[self.active].panes[self.tabs[self.active].focused]
            .session
            .begin_terminal_zmodem();
        match grab {
            Ok(stream) => {
                if let Some(files) = self.files.as_mut() {
                    files.start_terminal_zmodem(stream, upload, local, ctx);
                }
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }
    /// Claims the window's outer few pixels for resizing.
    ///
    /// A frameless window gets no resize border from the OS, and winit does not add
    /// one — so without this the window simply cannot be resized by dragging, which
    /// is what it did before. The edges are detected here and handed to the
    /// platform, which then runs its own resize loop.
    fn resize_grips(&self, ctx: &egui::Context) {
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
    fn topbar(&mut self, ctx: &egui::Context, action: &mut Option<Action>) {
        let p = self.palette;
        egui::TopBottomPanel::top("topbar")
            .frame(
                egui::Frame::new()
                    .fill(p.panel)
                    .inner_margin(egui::Margin::symmetric(4, 2)),
            )
            .show(ctx, |ui| {
                let mut close = false;
                let mut toggle = false;
                // Read maximized fresh every frame: the window can also be maximized
                // by the OS (snap, Win+Up, taskbar) without going through a button.
                let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                // The tab row and the window buttons share one bar, so the frameless
                // chrome costs a single row of height instead of two.
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
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
                    // Derive the edge from the buttons themselves; `min_rect()` would
                    // also fold in unrelated widgets. The drag strip below is
                    // registered later and would win any overlap, so this boundary
                    // has to be exact.
                    let buttons_left = close_button
                        .rect
                        .union(toggle_button.rect)
                        .union(minimize_button.rect)
                        .left()
                        - 4.0;
                    if close_button.clicked() {
                        close = true;
                    }
                    if toggle_button.clicked() {
                        toggle = true;
                    }
                    if minimize_button.clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.menu_button(RichText::new("G  菜单").color(p.accent), |ui| {
                            for (icon, text, shortcut, a) in [
                                (
                                    icons::Icon::Terminal,
                                    "新建终端",
                                    Some("Ctrl+Shift+T"),
                                    Action::New(SessionKind::Local(
                                        self.settings.default_shell.clone(),
                                    )),
                                ),
                                (icons::Icon::Host, "新建 SSH 连接", None, Action::Remote),
                                (icons::Icon::File, "文件与传输队列", None, Action::Files),
                                (
                                    icons::Icon::SplitHorizontal,
                                    "左右分屏",
                                    Some("Ctrl+Shift+D"),
                                    Action::Split(Axis::Horizontal),
                                ),
                                (
                                    icons::Icon::SplitVertical,
                                    "上下分屏",
                                    Some("Ctrl+Shift+E"),
                                    Action::Split(Axis::Vertical),
                                ),
                                (
                                    icons::Icon::ClosePane,
                                    "关闭窗格",
                                    Some("Ctrl+Shift+W"),
                                    Action::ClosePane,
                                ),
                                (
                                    icons::Icon::Restart,
                                    "重新连接 / 重启",
                                    None,
                                    Action::Restart,
                                ),
                            ] {
                                if icons::icon_row(ui, icon, text, shortcut, p).clicked() {
                                    *action = Some(a);
                                    ui.close();
                                }
                            }
                            if icons::icon_row(
                                ui,
                                icons::Icon::Search,
                                "查找历史",
                                Some("Ctrl+Shift+F"),
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
                            if icons::icon_row(
                                ui,
                                icons::Icon::Settings,
                                "偏好设置",
                                Some("Ctrl+,"),
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
                        if icons::icon_button(
                            ui,
                            if self.settings.sidebar {
                                icons::Icon::ChevronLeft
                            } else {
                                icons::Icon::ChevronRight
                            },
                            p,
                            icons::Size::Button,
                        )
                        .on_hover_text("展开 / 收起导航栏")
                        .clicked()
                        {
                            self.settings.sidebar = !self.settings.sidebar;
                        }
                        ui.separator();
                        // Bound the strip so a stretch of empty bar always remains for
                        // dragging the frameless window by.
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
                                            ui.push_id(t.id, |ui| {
                                                egui::Frame::new()
                                                    .fill(if i == self.active {
                                                        p.raised
                                                    } else {
                                                        p.panel
                                                    })
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
                                                                let text = pane
                                                                    .session
                                                                    .kind
                                                                    .label()
                                                                    .chars()
                                                                    .take(18)
                                                                    .collect::<String>();
                                                                ui.label(if focused {
                                                                    RichText::new(text)
                                                                        .color(color)
                                                                        .strong()
                                                                } else {
                                                                    RichText::new(text).color(color)
                                                                });
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
                                                            .interact(
                                                                rect,
                                                                ui.id().with("tab-hit"),
                                                                Sense::click(),
                                                            )
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
                                                            for (text, a) in [
                                                                (
                                                                    "左右分屏",
                                                                    Action::Split(Axis::Horizontal),
                                                                ),
                                                                (
                                                                    "上下分屏",
                                                                    Action::Split(Axis::Vertical),
                                                                ),
                                                                ("关闭标签", Action::CloseTab(i)),
                                                            ] {
                                                                if ui.button(text).clicked() {
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
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
    }
    fn sidebar(&mut self, ctx: &egui::Context, action: &mut Option<Action>) {
        let p = self.palette;
        egui::SidePanel::left("navigation")
            .default_width(185.0)
            .width_range(140.0..=320.0)
            .frame(egui::Frame::new().fill(p.panel).inner_margin(5))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::CollapsingHeader::new(RichText::new("本地 Shell").color(p.muted))
                        .default_open(true)
                        .show(ui, |ui| {
                            for (key, label) in if cfg!(windows) {
                                vec![
                                    ("powershell", "PowerShell"),
                                    ("pwsh", "PowerShell 7"),
                                    ("cmd", "CMD"),
                                    ("wsl", "WSL"),
                                ]
                            } else {
                                vec![("shell", "Shell")]
                            } {
                                if icons::icon_row(ui, icons::Icon::Terminal, label, None, p)
                                    .clicked()
                                {
                                    *action = Some(Action::New(SessionKind::Local(key.into())));
                                }
                            }
                        });
                    ui.horizontal(|ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(15.0, 15.0), Sense::hover());
                        icons::draw(ui.painter(), rect, icons::Icon::Host, p.muted, 1.2);
                        ui.label(hint("连接", p));
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
                    for profile in &self.settings.profiles {
                        if !profile.group.is_empty() && !groups.contains(&profile.group) {
                            groups.push(profile.group.clone());
                        }
                    }
                    groups.insert(0, String::new());
                    for group in groups {
                        let count = self
                            .settings
                            .profiles
                            .iter()
                            .filter(|p| p.group == group)
                            .count();
                        if group.is_empty() && count == 0 && !self.settings.profiles.is_empty() {
                            continue;
                        }
                        egui::CollapsingHeader::new(format!(
                            "{} ({count})",
                            if group.is_empty() {
                                "未分组"
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
                                .filter(|(_, p)| p.group == group)
                            {
                                let r = icons::icon_row(
                                    ui,
                                    icons::Icon::Host,
                                    &profile.label(),
                                    None,
                                    p,
                                );
                                if r.double_clicked() {
                                    *action = Some(Action::New(SessionKind::Ssh(profile.clone())));
                                }
                                r.on_hover_text(format!(
                                    "{}:{} · 双击连接 / 右键管理",
                                    profile.destination(),
                                    profile.port
                                ))
                                .context_menu(|ui| {
                                    for (text, a) in [
                                        (
                                            "连接 SSH",
                                            Action::New(SessionKind::Ssh(profile.clone())),
                                        ),
                                        ("编辑 / 跳板机 / 转发", Action::Edit(index)),
                                        ("删除连接", Action::Remove(index)),
                                    ] {
                                        if ui.button(text).clicked() {
                                            *action = Some(a);
                                            ui.close();
                                        }
                                    }
                                });
                            }
                        });
                    }
                });
            });
    }
    fn dialogs(&mut self, ctx: &egui::Context, action: &mut Option<Action>) {
        let p = self.palette;
        let mut open = self.settings_open;
        let mut changed = false;
        egui::Window::new("偏好设置")
            .open(&mut open)
            .collapsible(false)
            .default_width(440.0)
            .show(ctx, |ui| {
                changed |= ui
                    .add(egui::Slider::new(&mut self.settings.font_size, 10.0..=28.0).text("字号"))
                    .changed();
                changed |= ui
                    .add(
                        egui::Slider::new(&mut self.settings.scrollback, 100..=50_000)
                            .logarithmic(true)
                            .text("历史行数（新会话）"),
                    )
                    .changed();
                ui.horizontal(|ui| {
                    changed |= ui
                        .checkbox(&mut self.settings.light_theme, "浅色主题")
                        .changed();
                    changed |= ui.checkbox(&mut self.settings.sidebar, "导航栏").changed();
                });
                changed |= ui
                    .checkbox(&mut self.settings.copy_on_select, "选中文本后自动复制")
                    .changed();
                changed |= ui
                    .checkbox(
                        &mut self.settings.hide_dotfiles,
                        "文件窗口默认隐藏 . 开头文件",
                    )
                    .changed();
                changed |= ui
                    .checkbox(
                        &mut self.settings.restore_workspace,
                        "启动时恢复标签与分屏布局",
                    )
                    .changed();
                ui.label(hint(
                    "鼠标中键：粘贴；Shift+鼠标：绕过应用鼠标协议选择文本。",
                    p,
                ));
                ui.label(hint(
                    "恢复会新建 Shell；远程会话要求重新认证，不恢复进程。",
                    p,
                ));
                ui.horizontal(|ui| {
                    ui.label("默认 Shell");
                    egui::ComboBox::from_id_salt("default-shell")
                        .selected_text(&self.settings.default_shell)
                        .show_ui(ui, |ui| {
                            for shell in if cfg!(windows) {
                                vec!["powershell", "pwsh", "cmd", "wsl"]
                            } else {
                                vec!["shell"]
                            } {
                                changed |= ui
                                    .selectable_value(
                                        &mut self.settings.default_shell,
                                        shell.into(),
                                        shell,
                                    )
                                    .changed();
                            }
                        });
                });
                ui.separator();
                ui.label(hint(Settings::path().to_string_lossy().as_ref(), p));
            });
        self.settings_open = open;
        if changed {
            self.palette = Palette::new(self.settings.light_theme);
            self.palette.apply(ctx, self.settings.light_theme);
            self.persist();
        }
        let mut open = self.remote_open;
        let mut save = false;
        let mut connect = false;
        egui::Window::new("连接配置")
            .open(&mut open)
            .collapsible(false)
            .default_width(480.0)
            .show(ctx, |ui| {
                egui::Grid::new("connection-form")
                    .spacing([12.0, 6.0])
                    .min_col_width(72.0)
                    .show(ui, |ui| {
                        ui.label("主机 / IP");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.remote.host).desired_width(280.0),
                        );
                        ui.end_row();
                        ui.label("端口");
                        ui.add(egui::DragValue::new(&mut self.remote.port).range(1..=65535));
                        ui.end_row();
                        ui.label("连接名称");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.remote.name).desired_width(280.0),
                        );
                        ui.end_row();
                        ui.label("分组");
                        egui::ComboBox::from_id_salt("profile-group")
                            .width(200.0)
                            .selected_text(if self.remote.group.is_empty() {
                                "未分组"
                            } else {
                                &self.remote.group
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.remote.group,
                                    String::new(),
                                    "未分组",
                                );
                                for group in &self.settings.groups {
                                    ui.selectable_value(
                                        &mut self.remote.group,
                                        group.clone(),
                                        group,
                                    );
                                }
                            });
                        ui.end_row();
                        ui.label("用户名");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.remote.user).desired_width(280.0),
                        );
                        ui.end_row();
                        ui.label("私钥路径");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.remote.identity)
                                .desired_width(280.0),
                        );
                        ui.end_row();
                    });
                egui::CollapsingHeader::new("ProxyJump 跳板机").show(ui, |ui| {
                    let mut enabled = self.remote.jump.is_some();
                    if ui.checkbox(&mut enabled, "启用一级跳板机").changed() {
                        self.remote.jump = if enabled {
                            Some(Box::new(RemoteProfile::default()))
                        } else {
                            None
                        };
                    }
                    if let Some(j) = &mut self.remote.jump {
                        egui::Grid::new("jump-form")
                            .spacing([12.0, 6.0])
                            .min_col_width(48.0)
                            .show(ui, |ui| {
                                ui.label("地址");
                                ui.add(
                                    egui::TextEdit::singleline(&mut j.host).desired_width(280.0),
                                );
                                ui.end_row();
                                ui.label("端口");
                                ui.add(egui::DragValue::new(&mut j.port).range(1..=65535));
                                ui.end_row();
                                ui.label("用户名");
                                ui.add(
                                    egui::TextEdit::singleline(&mut j.user).desired_width(280.0),
                                );
                                ui.end_row();
                                ui.label("私钥");
                                ui.add(
                                    egui::TextEdit::singleline(&mut j.identity)
                                        .desired_width(280.0),
                                );
                                ui.end_row();
                            });
                    }
                });
                egui::CollapsingHeader::new("本地端口转发（监听 127.0.0.1）").show(ui, |ui| {
                    let mut remove = None;
                    for (i, f) in self.remote.forwards.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut f.bind_port).range(1..=65535));
                            ui.label("→");
                            ui.add(
                                egui::TextEdit::singleline(&mut f.target_host).desired_width(160.0),
                            );
                            ui.add(egui::DragValue::new(&mut f.target_port).range(1..=65535));
                            if ui.small_button("×").clicked() {
                                remove = Some(i);
                            }
                        });
                    }
                    if let Some(i) = remove {
                        self.remote.forwards.remove(i);
                    }
                    if ui.small_button("+ 转发规则").clicked() {
                        self.remote.forwards.push(Forward {
                            bind_port: 8080,
                            target_host: "127.0.0.1".into(),
                            target_port: 80,
                        });
                    }
                });
                ui.label(hint("认证在连接时进行，密码不写入配置。", p));
                ui.horizontal(|ui| {
                    if ui.button("保存").clicked() {
                        save = true;
                    }
                    if ui.button("保存并连接").clicked() {
                        save = true;
                        connect = true;
                    }
                });
            });
        self.remote_open = open;
        if save {
            match self.remote.validate() {
                Ok(()) => {
                    if let Some(i) = self.editing_profile {
                        self.settings.profiles[i] = self.remote.clone();
                    } else {
                        self.settings.profiles.push(self.remote.clone());
                    }
                    self.remote_open = false;
                    self.persist();
                    if connect {
                        *action = Some(Action::New(SessionKind::Ssh(self.remote.clone())));
                    }
                }
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        let mut open = self.groups_open;
        let mut modified = false;
        egui::Window::new("连接分组")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.new_group).desired_width(180.0));
                    if ui.button("添加").clicked() {
                        let name = self.new_group.trim().to_string();
                        if !name.is_empty() && !self.settings.groups.contains(&name) {
                            self.settings.groups.push(name);
                            modified = true;
                            self.new_group.clear();
                        }
                    }
                });
                let mut remove = None;
                let mut rename = None;
                for (i, group) in self.settings.groups.iter_mut().enumerate() {
                    let old = group.clone();
                    ui.horizontal(|ui| {
                        if ui.text_edit_singleline(group).changed() {
                            rename = Some((old.clone(), group.clone()));
                        }
                        if ui.small_button("删除").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some((old, new)) = rename {
                    for p in &mut self.settings.profiles {
                        if p.group == old {
                            p.group = new.clone();
                        }
                    }
                    modified = true;
                }
                if let Some(i) = remove {
                    let group = self.settings.groups.remove(i);
                    for p in &mut self.settings.profiles {
                        if p.group == group {
                            p.group.clear();
                        }
                    }
                    modified = true;
                }
                ui.label(hint("删除分组后，连接移到未分组。", p));
            });
        self.groups_open = open;
        if modified {
            self.persist();
        }
        egui::Window::new("关于 / 快捷键")
            .open(&mut self.help_open)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(concat!(
                        "G-Terminal ",
                        env!("CARGO_PKG_VERSION"),
                        " · Native. Fast. Yours."
                    ))
                    .color(p.accent),
                );
                for (key, description) in [
                    ("Ctrl+Shift+T / W", "新建标签 / 关闭窗格"),
                    ("Ctrl+Shift+D / E", "左右 / 上下分屏"),
                    ("Ctrl+Tab / Alt+Right", "切换标签 / 窗格"),
                    ("Ctrl+Shift+C / V", "复制 / 粘贴"),
                    ("Ctrl+C", "终端中断"),
                    ("Ctrl+Shift+F", "全部保留历史查找"),
                    ("Ctrl+Shift+B / Ctrl+,", "导航栏 / 设置"),
                    ("中键 / Shift+鼠标", "粘贴 / 强制选择"),
                    ("Ctrl+Plus / Minus / 0", "字号放大 / 缩小 / 重置"),
                ] {
                    ui.horizontal(|ui| {
                        ui.monospace(key);
                        ui.label(description);
                    });
                }
                ui.separator();
                ui.label(hint(
                    "Agent 仅预留协议。终端兼容性范围和待实现项见 README。",
                    p,
                ));
            });
        if let Some(login) = &mut self.login {
            let mut open = true;
            let ready = login.show(ctx, &mut open, p);
            if let Some(c) = ready {
                self.login = None;
                self.add_connection(c, ctx);
            } else if !open {
                self.login = None;
            }
        }
        if let Some(files) = &mut self.files {
            files.show(ctx, &mut self.files_open, p, self.settings.hide_dotfiles);
            // A running transfer has to keep ticking so its progress and any
            // conflict prompt show up while the user is idle.
            if files.busy() {
                ctx.request_repaint_after(std::time::Duration::from_millis(150));
            }
        }
    }
    fn find(&mut self, next: bool) {
        if let Some(t) = self.tabs.get_mut(self.active) {
            if !next {
                self.search_hits = t.panes[t.focused]
                    .session
                    .terminal
                    .lock()
                    .unwrap()
                    .find_all(&self.search);
                self.search_index = 0;
            } else if !self.search_hits.is_empty() {
                self.search_index = (self.search_index + 1) % self.search_hits.len();
            }
            if let Some((offset, _)) = self.search_hits.get(self.search_index) {
                t.panes[t.focused]
                    .session
                    .terminal
                    .lock()
                    .unwrap()
                    .parser
                    .screen_mut()
                    .set_scrollback(*offset);
            }
        }
    }
    pub(crate) fn render(&mut self, ctx: &egui::Context) {
        let p = self.palette;
        self.resize_grips(ctx);
        let mut action = self.shortcuts(ctx);
        self.topbar(ctx, &mut action);
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
                                if s.remote.is_some() { "SSH" } else { "ConPTY" },
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
                                action = Some(Action::Files);
                            }
                        }
                    } else {
                        ui.label(hint("就绪", p));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
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
        if self.settings.sidebar {
            self.sidebar(ctx, &mut action);
        }
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
                        if cfg!(windows) {
                            for (key, label) in [
                                ("powershell", "本地 PowerShell"),
                                ("pwsh", "本地 PowerShell 7"),
                                ("cmd", "本地 CMD"),
                                ("wsl", "本地 WSL"),
                            ] {
                                if ui.button(label).clicked() {
                                    chosen = Some(SessionKind::Local(key.into()));
                                }
                            }
                        } else if ui.button("本地 Shell").clicked() {
                            chosen = Some(SessionKind::Local("shell".into()));
                        }
                        ui.separator();
                        for profile in &self.settings.profiles {
                            if ui.button(format!("SSH · {}", profile.label())).clicked() {
                                chosen = Some(SessionKind::Ssh(profile.clone()));
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
                                self.login = Some(remote_ui::Login::new(profile));
                            }
                            kind => {
                                if let Some(pane) = self.spawn(kind, ctx) {
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
        if let Some(t) = self.tabs.get(self.active)
            && let Some(c) = t.panes[t.focused].session.remote.clone()
        {
            // The ZMODEM frame type decides the direction, so the old screen-text
            // guess ("waiting to receive") is gone — and with it the reason `rz`
            // never raised an offer at all.
            let offer = self.tabs[self.active].panes[self.tabs[self.active].focused]
                .session
                .terminal
                .lock()
                .unwrap()
                .zmodem_offer;
            if let Some(upload) = offer {
                // The picker opens straight away. The confirmation that used to sit
                // in front of it decided nothing: if you type `rz` or `sz` you have
                // already said what you want, and the picker's own cancel button is
                // the way to back out.
                self.tabs[self.active].panes[self.tabs[self.active].focused]
                    .session
                    .terminal
                    .lock()
                    .unwrap()
                    .zmodem_offer = None;
                let local = if upload {
                    rfd::FileDialog::new().pick_file()
                } else {
                    rfd::FileDialog::new().pick_folder()
                };
                match local {
                    Some(local) => self.begin_terminal_zmodem(upload, local, &c, ctx),
                    // Backing out of the picker is the only way left to refuse, so
                    // abort the lrzsz that is waiting on the other end.
                    None => {
                        let _ = self.tabs[self.active].panes[self.tabs[self.active].focused]
                            .session
                            .write(vec![0x18; 8]);
                    }
                }
            }
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg).inner_margin(0))
            .show(ctx, |ui| {
                if self.search_open {
                    ui.horizontal(|ui| {
                        ui.label(hint("历史", p));
                        let r = ui
                            .add(egui::TextEdit::singleline(&mut self.search).desired_width(260.0));
                        if self.search_focus {
                            r.request_focus();
                            self.search_focus = false;
                        }
                        if ui.button("查找").clicked()
                            || (r.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
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
                    && !self.search_open
                    && self.login.is_none()
                    && !self.files_open
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
                                            action = Some(Action::Restart);
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
                                    },
                                );
                                if clicked {
                                    focus = index;
                                }
                                if error.is_some() {
                                    self.error = error;
                                }
                            },
                        );
                    }
                    t.focused = focus;
                } else {
                    ui.centered_and_justified(|ui| {
                        if ui.button("+ 新建终端   Ctrl+Shift+T").clicked() {
                            action = Some(Action::New(SessionKind::Local(
                                self.settings.default_shell.clone(),
                            )));
                        }
                    });
                }
            });
        self.dialogs(ctx, &mut action);
        if let Some(files) = &mut self.files {
            files.show_question(ctx, p);
            if files.busy() || files.loading() {
                ctx.request_repaint_after(std::time::Duration::from_millis(150));
            }
        }
        if let Some(action) = action {
            self.execute(action, ctx);
        }
        if let Some(path) = self.screenshot.clone() {
            for e in ctx.input(|i| i.events.clone()) {
                if let egui::Event::Screenshot { image, .. } = e {
                    if let Err(e) = image::save_buffer(
                        &path,
                        image.as_raw(),
                        image.width() as u32,
                        image.height() as u32,
                        image::ColorType::Rgba8,
                    ) {
                        eprintln!("Screenshot: {e}");
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            if !self.screenshot_requested && self.started.elapsed().as_secs_f32() > 4.0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
                self.screenshot_requested = true;
            }
            if self.started.elapsed().as_secs() > 20 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.render(ctx);
    }
    fn on_exit(&mut self) {
        if self.screenshot.is_none() {
            self.persist();
        }
    }
}
/// A tab shows the state of its least healthy pane.
fn tab_link(tab: &Tab) -> SessionStatus {
    let mut worst = SessionStatus::Live;
    for pane in &tab.panes {
        match pane.session.link() {
            SessionStatus::Lost => return SessionStatus::Lost,
            SessionStatus::Detached => worst = SessionStatus::Detached,
            SessionStatus::Live => {}
        }
    }
    worst
}

/// Which frame icon a titlebar button paints. `Restore` and `Maximize` are the
/// same button in two states.
#[derive(Clone, Copy, PartialEq)]
enum TitleButton {
    Close,
    Maximize,
    Restore,
    Minimize,
}

/// The window-chrome buttons. Painted through the shared icon set, for the same
/// reason every other glyph is: no font in the loaded chain carries these
/// codepoints, so the text versions rendered as tofu boxes.
fn title_button(ui: &mut egui::Ui, kind: TitleButton, p: Palette) -> egui::Response {
    let icon = match kind {
        TitleButton::Close => icons::Icon::Close,
        TitleButton::Maximize => icons::Icon::Maximize,
        TitleButton::Restore => icons::Icon::Restore,
        TitleButton::Minimize => icons::Icon::Minimize,
    };
    icons::window_button(ui, icon, p, kind == TitleButton::Close)
}

fn link_color(link: SessionStatus, p: Palette) -> egui::Color32 {
    match link {
        SessionStatus::Live => p.ok,
        SessionStatus::Detached => p.muted,
        SessionStatus::Lost => p.danger,
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, RawInput, Rect, Vec2};

    #[test]
    fn nested_workspace_roundtrips_without_persisting_authentication() {
        let ctx = egui::Context::default();
        let mut app = App::from_settings(&ctx, Settings::default(), None, None);
        app.execute(Action::Split(Axis::Horizontal), &ctx);
        app.execute(Action::Split(Axis::Vertical), &ctx);
        assert_eq!(app.tabs[0].panes.len(), 3);
        assert!(app.tabs[0].layout.valid(3));
        app.snapshot();
        let config = serde_json::to_string(&app.settings).unwrap();
        assert!(!config.contains("password"));
        let settings: Settings = serde_json::from_str(&config).unwrap();
        drop(app);
        let restored = App::from_settings(&ctx, settings, None, None);
        assert_eq!(restored.tabs[0].panes.len(), 3);
        assert!(restored.tabs[0].layout.valid(3));
    }

    fn frame(
        app: &mut App,
        ctx: &egui::Context,
        events: Vec<Event>,
        modifiers: Modifiers,
        size: Vec2,
    ) {
        let _ = ctx.run(
            RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
                modifiers,
                events,
                ..Default::default()
            },
            |ctx| app.render(ctx),
        );
    }

    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn workspace_routes_keyboard_splits_and_dialogs_without_leaking_commands() {
        let ctx = egui::Context::default();
        let mut app = App::from_settings(&ctx, Settings::default(), None, None);
        let size = Vec2::new(1280.0, 800.0);
        let mods = Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
        frame(&mut app, &ctx, vec![key(Key::T, mods)], mods, size);
        assert_eq!(app.tabs.len(), 2);
        frame(&mut app, &ctx, vec![key(Key::D, mods)], mods, size);
        assert_eq!(app.tabs[1].panes.len(), 2);
        assert_eq!(app.tabs[1].focused, 1);
        frame(
            &mut app,
            &ctx,
            vec![],
            Modifiers::NONE,
            Vec2::new(760.0, 480.0),
        );
        let start = std::time::Instant::now();
        while !app.tabs[1].panes[1]
            .session
            .terminal
            .lock()
            .unwrap()
            .parser
            .screen()
            .contents()
            .contains("PS ")
        {
            assert!(start.elapsed().as_secs() < 15);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        frame(
            &mut app,
            &ctx,
            vec![
                Event::Text("Write-Output ('GUI_' + 'INPUT_OK')".into()),
                key(Key::Enter, Modifiers::NONE),
            ],
            Modifiers::NONE,
            size,
        );
        let start = std::time::Instant::now();
        while !app.tabs[1].panes[1]
            .session
            .terminal
            .lock()
            .unwrap()
            .parser
            .screen()
            .contents()
            .contains("GUI_INPUT_OK")
        {
            assert!(
                start.elapsed().as_secs() < 10,
                "Keyboard did not reach active pane"
            );
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        assert!(
            !app.tabs[1].panes[0]
                .session
                .terminal
                .lock()
                .unwrap()
                .parser
                .screen()
                .contents()
                .contains("GUI_INPUT_OK")
        );
        frame(&mut app, &ctx, vec![key(Key::F, mods)], mods, size);
        assert!(app.search_open);
        frame(
            &mut app,
            &ctx,
            vec![key(Key::Escape, Modifiers::NONE)],
            Modifiers::NONE,
            size,
        );
        assert!(!app.search_open);
        frame(&mut app, &ctx, vec![key(Key::W, mods)], mods, size);
        assert_eq!(app.tabs[1].panes.len(), 1);
        frame(&mut app, &ctx, vec![key(Key::W, mods)], mods, size);
        assert_eq!(app.tabs.len(), 1);
        assert!(app.error.is_none(), "{:?}", app.error);
    }

    /// Runs one frame and collects the viewport commands it produced, so window
    /// chrome behaviour can be asserted instead of eyeballed.
    fn commands(
        app: &mut App,
        ctx: &egui::Context,
        events: Vec<Event>,
        size: Vec2,
    ) -> Vec<egui::ViewportCommand> {
        let output = ctx.run(
            RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ctx| app.render(ctx),
        );
        output
            .viewport_output
            .into_values()
            .flat_map(|viewport| viewport.commands)
            .collect()
    }

    fn press(pos: egui::Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    /// The merged bar has to keep dragging the frameless window by its empty area.
    /// That is the whole reason the tab row and the window buttons can share one
    /// panel, so it is worth asserting rather than assuming.
    #[test]
    fn dragging_the_empty_bar_starts_a_window_drag() {
        let ctx = egui::Context::default();
        let mut app = App::from_settings(&ctx, Settings::default(), None, None);
        let size = Vec2::new(1280.0, 800.0);
        // The drag rect must exist for a frame before egui can hit-test against it.
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
        // Well clear of the menu, the tabs and the three window buttons.
        let empty = egui::Pos2::new(640.0, 13.0);
        frame(
            &mut app,
            &ctx,
            vec![Event::PointerMoved(empty), press(empty, true)],
            Modifiers::NONE,
            size,
        );
        let mut seen = Vec::new();
        for step in 1..=3 {
            seen.extend(commands(
                &mut app,
                &ctx,
                vec![Event::PointerMoved(
                    empty + Vec2::new(30.0 * step as f32, 0.0),
                )],
                size,
            ));
        }
        assert!(
            seen.contains(&egui::ViewportCommand::StartDrag),
            "the empty bar no longer drags the window: {seen:?}"
        );
    }

    /// The drag strip is registered after the buttons, so it wins any overlap in
    /// hit-testing. This pins the boundary that keeps it off their clicks.
    #[test]
    fn window_buttons_are_not_swallowed_by_the_drag_strip() {
        let ctx = egui::Context::default();
        let mut app = App::from_settings(&ctx, Settings::default(), None, None);
        let size = Vec2::new(1280.0, 800.0);
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
        // Right to left from the panel's 4pt inner margin: close, then maximize.
        let maximize = egui::Pos2::new(size.x - 4.0 - 32.0 - 4.0 - 16.0, 13.0);
        frame(
            &mut app,
            &ctx,
            vec![Event::PointerMoved(maximize), press(maximize, true)],
            Modifiers::NONE,
            size,
        );
        let seen = commands(&mut app, &ctx, vec![press(maximize, false)], size);
        assert!(
            seen.iter()
                .any(|c| matches!(c, egui::ViewportCommand::Maximized(_))),
            "the maximize button produced {seen:?}"
        );
        assert!(
            !seen.contains(&egui::ViewportCommand::StartDrag),
            "the drag strip swallowed the button click"
        );
    }

    /// A frameless window gets no resize border from the OS, and winit does not
    /// add one, so the edges are claimed in-app. This pins that a press on the
    /// border asks the platform for a resize — and that a press away from every
    /// border does not, or ordinary clicking would start dragging the frame.
    #[test]
    fn pressing_a_window_edge_asks_the_platform_to_resize() {
        let ctx = egui::Context::default();
        let mut app = App::from_settings(&ctx, Settings::default(), None, None);
        let size = Vec2::new(1280.0, 800.0);
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);

        // Just inside the left edge.
        let edge = egui::Pos2::new(2.0, 400.0);
        let seen = commands(
            &mut app,
            &ctx,
            vec![Event::PointerMoved(edge), press(edge, true)],
            size,
        );
        assert!(
            seen.iter()
                .any(|command| matches!(command, egui::ViewportCommand::BeginResize(_))),
            "the window edge did not start a resize: {seen:?}"
        );

        // Well clear of every edge.
        let middle = egui::Pos2::new(640.0, 400.0);
        let seen = commands(
            &mut app,
            &ctx,
            vec![Event::PointerMoved(middle), press(middle, false)],
            size,
        );
        assert!(
            !seen
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::BeginResize(_))),
            "a click in the middle started a resize: {seen:?}"
        );
    }
}
