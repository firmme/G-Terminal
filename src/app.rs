use crate::{
    remote_ui::{self, Files, Login, hint},
    theme::{Palette, load_fonts},
    view::Pane,
};
use eframe::egui::{self, Align, Button, Key, Layout, Rect, RichText, Sense};
use g_terminal::{
    config::{Forward, RemoteProfile, Settings},
    layout::{Axis, Layout as PaneLayout, SavedTab},
    remote::Connection,
    session::{Session, SessionKind},
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
                        let t = &mut self.tabs[self.active];
                        t.layout.split(t.focused, t.panes.len(), axis);
                        t.focused = t.panes.len();
                        t.panes.push(p);
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
                        self.files = Some(Files::new(c, ctx));
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
    fn topbar(&mut self, ctx: &egui::Context, action: &mut Option<Action>) {
        let p = self.palette;
        egui::TopBottomPanel::top("topbar")
            .frame(
                egui::Frame::new()
                    .fill(p.panel)
                    .inner_margin(egui::Margin::symmetric(4, 2)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.menu_button(RichText::new("G  菜单").color(p.accent), |ui| {
                        for (text, a) in [
                            (
                                "新建终端           Ctrl+Shift+T",
                                Action::New(SessionKind::Local(
                                    self.settings.default_shell.clone(),
                                )),
                            ),
                            ("新建 SSH 连接", Action::Remote),
                            ("文件与传输队列", Action::Files),
                            (
                                "左右分屏           Ctrl+Shift+D",
                                Action::Split(Axis::Horizontal),
                            ),
                            (
                                "上下分屏           Ctrl+Shift+E",
                                Action::Split(Axis::Vertical),
                            ),
                            ("关闭窗格           Ctrl+Shift+W", Action::ClosePane),
                            ("重新连接 / 重启", Action::Restart),
                        ] {
                            if ui.button(text).clicked() {
                                *action = Some(a);
                                ui.close();
                            }
                        }
                        if ui.button("查找历史           Ctrl+Shift+F").clicked() {
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
                        if ui.button("连接分组管理").clicked() {
                            self.groups_open = true;
                            ui.close();
                        }
                        if ui.button("偏好设置           Ctrl+,").clicked() {
                            self.settings_open = true;
                            ui.close();
                        }
                        if ui.button("快捷键 / 关于").clicked() {
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
                    if ui
                        .small_button(if self.settings.sidebar { "‹" } else { "›" })
                        .on_hover_text("展开 / 收起导航栏")
                        .clicked()
                    {
                        self.settings.sidebar = !self.settings.sidebar;
                    }
                    ui.separator();
                    egui::ScrollArea::horizontal()
                        .id_salt("tab-strip")
                        .max_width((ui.available_width() - 30.0).max(100.0))
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                for (i, t) in self.tabs.iter().enumerate() {
                                    ui.push_id(t.id, |ui| {
                                        egui::Frame::new()
                                            .fill(if i == self.active { p.raised } else { p.panel })
                                            .inner_margin(egui::Margin::symmetric(6, 0))
                                            .show(ui, |ui| {
                                                ui.horizontal(|ui| {
                                                    let label =
                                                        t.panes[t.focused].session.kind.label();
                                                    let r = ui.add(
                                                        Button::new(
                                                            RichText::new(
                                                                label
                                                                    .chars()
                                                                    .take(30)
                                                                    .collect::<String>(),
                                                            )
                                                            .color(if i == self.active {
                                                                p.text
                                                            } else {
                                                                p.muted
                                                            }),
                                                        )
                                                        .frame(false),
                                                    );
                                                    if r.clicked() {
                                                        self.active = i;
                                                    }
                                                    if r.clicked_by(egui::PointerButton::Middle) {
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
                                                    if ui
                                                        .add(
                                                            Button::new(
                                                                RichText::new("×").color(p.muted),
                                                            )
                                                            .frame(false),
                                                        )
                                                        .clicked()
                                                    {
                                                        *action = Some(Action::CloseTab(i));
                                                    }
                                                });
                                            });
                                    });
                                }
                            });
                        });
                    if ui.small_button("+").clicked() {
                        *action = Some(Action::New(SessionKind::Local(
                            self.settings.default_shell.clone(),
                        )));
                    }
                });
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
                                if ui
                                    .add_sized(
                                        [ui.available_width(), 22.0],
                                        Button::new(label).frame(false),
                                    )
                                    .clicked()
                                {
                                    *action = Some(Action::New(SessionKind::Local(key.into())));
                                }
                            }
                        });
                    ui.horizontal(|ui| {
                        ui.label(hint("连接", p));
                        if ui.small_button("+").clicked() {
                            *action = Some(Action::Remote);
                        }
                        if ui.small_button("分组").clicked() {
                            self.groups_open = true;
                        }
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
                                let r = ui.add_sized(
                                    [ui.available_width(), 22.0],
                                    Button::new(profile.label()).frame(false),
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
                    .spacing([14.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("主机 / IP");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.remote.host)
                                .hint_text(hint("192.168.1.10", p)),
                        );
                        ui.end_row();
                        ui.label("连接名称");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.remote.name).hint_text(hint(
                                if self.remote.host.is_empty() {
                                    "留空使用 IP / 主机名"
                                } else {
                                    &self.remote.host
                                },
                                p,
                            )),
                        );
                        ui.end_row();
                        ui.label("分组");
                        egui::ComboBox::from_id_salt("profile-group")
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
                        ui.label("用户名 / 端口");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.remote.user)
                                    .desired_width(155.0)
                                    .hint_text(hint("系统用户名", p)),
                            );
                            ui.add(egui::DragValue::new(&mut self.remote.port).range(1..=65535));
                        });
                        ui.end_row();
                        ui.label("私钥路径");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.remote.identity)
                                .hint_text(hint("可选，使用完整路径", p)),
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
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut j.host)
                                    .desired_width(170.0)
                                    .hint_text(hint("跳板机 IP", p)),
                            );
                            ui.add(egui::DragValue::new(&mut j.port).range(1..=65535));
                            ui.add(
                                egui::TextEdit::singleline(&mut j.user)
                                    .desired_width(110.0)
                                    .hint_text(hint("用户名", p)),
                            );
                        });
                        ui.add(
                            egui::TextEdit::singleline(&mut j.identity)
                                .hint_text(hint("跳板机私钥，可选", p)),
                        );
                    }
                });
                egui::CollapsingHeader::new("本地端口转发（监听 127.0.0.1）").show(ui, |ui| {
                    let mut remove = None;
                    for (i, f) in self.remote.forwards.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut f.bind_port).range(1..=65535));
                            ui.label("→");
                            ui.add(
                                egui::TextEdit::singleline(&mut f.target_host)
                                    .desired_width(160.0)
                                    .hint_text(hint("目标地址", p)),
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
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_group)
                            .hint_text(hint("新分组名称", p)),
                    );
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
            files.show(ctx, &mut self.files_open, p);
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
                        ui.colored_label(egui::Color32::LIGHT_RED, error);
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
                            ui.colored_label(egui::Color32::LIGHT_RED, e);
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
                            if ui.small_button("文件").clicked() {
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
            });
        if self.settings.sidebar {
            self.sidebar(ctx, &mut action);
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg).inner_margin(0))
            .show(ctx, |ui| {
                if self.search_open {
                    ui.horizontal(|ui| {
                        ui.label(hint("历史", p));
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut self.search)
                                .desired_width(220.0)
                                .hint_text(hint("搜索保留的全部历史…", p)),
                        );
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
}
