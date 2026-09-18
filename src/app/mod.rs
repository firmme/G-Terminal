use crate::{
    editing, icons,
    remote_ui::{self, Files, Login, format_size, hint},
    theme::{Palette, load_fonts},
    update,
    view::Pane,
};
use eframe::egui::{self, Align, Key, Layout, Rect, RichText, Sense};
use g_terminal::{
    config::{
        BAUD_RATES, DEFAULT_BAUD, Forward, RemoteProfile, SEARCH_ENGINES, SerialProfile, Settings,
        TAG_COLORS, search_engine_label,
    },
    layout::{Axis, Layout as PaneLayout, SavedTab},
    remote::Connection,
    serial,
    session::{Session, SessionKind, SessionStatus, local_shells, shell_label},
    terminal::Terminal,
};
use std::sync::{Arc, Mutex};

/// The accelerator shown in menus and the help window. egui maps
/// `Modifiers::command` to Cmd on macOS and Ctrl elsewhere, so the label has to
/// follow the same split.
const ACCEL: &str = if cfg!(target_os = "macos") {
    "Cmd"
} else {
    "Ctrl"
};

/// Rewrites the Windows-style shortcut text in this file for display.
pub(crate) fn accel(shortcut: &str) -> String {
    if ACCEL == "Ctrl" {
        shortcut.to_string()
    } else {
        shortcut.replace("Ctrl", ACCEL)
    }
}

mod dialogs;
mod panes;
mod port_owner;
use port_owner::*;
mod sidebar;
mod status;
mod titlebar;
mod transfer;
mod updates;
mod widgets;
use status::*;
use widgets::*;

struct Tab {
    id: u64,
    panes: Vec<Pane>,
    focused: usize,
    layout: PaneLayout,
    /// Bytes received by this tab's panes the last time it was on screen. A
    /// background tab whose panes have read more since is showing new output,
    /// which the strip underlines until it is looked at again.
    seen_output: u64,
}

/// Bytes received by every pane of a tab, which only ever grows while a session
/// is live.
fn tab_output(tab: &Tab) -> u64 {
    use std::sync::atomic::Ordering;
    tab.panes
        .iter()
        .map(|pane| pane.session.traffic.down.load(Ordering::Relaxed))
        .sum()
}

/// Whether a tab holds output the user has not looked at yet. The active tab is
/// being looked at by definition, so it is never marked.
fn tab_updated(tab: &Tab, active: bool) -> bool {
    !active && tab_output(tab) > tab.seen_output
}
#[derive(Clone)]
enum Action {
    New(SessionKind),
    /// Open another application window.
    NewWindow,
    /// Open a new tab with the same session as this tab's focused pane.
    DuplicateSession(usize),
    Split(Axis),
    CloseTab(usize),
    /// Close every tab except this one.
    CloseOtherTabs(usize),
    /// Close every tab whose sessions have all ended.
    CloseDisconnectedTabs,
    /// Look for a newer GitHub release.
    CheckUpdates,
    ClosePane,
    Restart,
    Disconnect,
    Remote,
    Edit(usize),
    Remove(usize),
    /// Copy a saved SSH connection.
    Duplicate(usize),
    /// Copy a saved serial connection.
    DuplicateSerial(usize),
    SerialPicker,
    /// Look for the program holding a serial port.
    FindPortOwner(String),
    EditSerial(usize),
    RemoveSerial(usize),
    /// Copy a host imported from `~/.ssh/config` into the saved connections.
    AdoptSshConfig(RemoteProfile),
    Toolbox,
    Files,
}

/// Which protocol the connection form is editing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProfileKind {
    Ssh,
    Serial,
}

/// State of the one-off serial picker opened from 本地 Shell.
struct SerialPicker {
    ports: Vec<serial::SerialPortInfo>,
    port: String,
    baud: u32,
}

impl SerialPicker {
    fn new() -> Self {
        let ports = serial::available_ports();
        let port = ports
            .first()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "auto".into());
        Self {
            ports,
            port,
            baud: DEFAULT_BAUD,
        }
    }

    /// Re-reads the device list, keeping the current choice when it is still
    /// attached.
    fn refresh(&mut self) {
        self.ports = serial::available_ports();
        // A manually typed device survives a refresh; only an empty field is
        // filled in from the list.
        if self.port.trim().is_empty() {
            self.port = self
                .ports
                .first()
                .map(|port| port.name.clone())
                .unwrap_or_else(|| "auto".into());
        }
    }
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
    serial: SerialProfile,
    /// Ports detected when the connection form or the picker was opened.
    serial_ports: Vec<serial::SerialPortInfo>,
    /// Set while the 本地 Shell serial picker is open.
    serial_picker: Option<SerialPicker>,
    /// Set while the 串口占用排查 window is open.
    port_owner: Option<PortOwnerWindow>,
    /// Set while the server toolbox is open.
    toolbox: Option<crate::toolbox::Toolbox>,
    /// Which tab of the 连接配置 window is in front.
    profile_kind: ProfileKind,
    editing_profile: Option<usize>,
    editing_serial: Option<usize>,
    new_group: String,
    search_open: bool,
    search: String,
    search_focus: bool,
    search_hits: Vec<(usize, u16)>,
    search_index: usize,
    /// A short message shown in the status bar, e.g. how much was copied.
    toast: Option<(String, std::time::Instant)>,
    rates: RateMeter,
    error: Option<String>,
    login: Option<Login>,
    login_target: Option<(u64, u64)>,
    files: Option<Files>,
    files_open: bool,
    split_chooser: Option<(u64, u64)>,
    screenshot: Option<std::path::PathBuf>,
    started: std::time::Instant,
    screenshot_requested: bool,
    /// In auto-hide mode, whether the bar has been pinned open by the topbar
    /// button instead of hiding until hovered.
    sidebar_pinned: bool,
    /// Whether the auto-hidden navigation bar is currently expanded.
    sidebar_reveal: bool,
    /// The floating navigation bar's rect from the last frame, so the pointer
    /// moving onto it keeps it open.
    sidebar_panel_rect: Option<Rect>,
    /// The exit confirmation is on screen.
    confirm_exit: bool,
    /// The user has already agreed to close with sessions open.
    exit_confirmed: bool,
    /// What the 检查更新 window shows; written by its worker thread.
    update_status: Arc<Mutex<update::Status>>,
    update_open: bool,
    /// The current OS window title, so it is only sent when it changes.
    window_title: String,
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
            serial: SerialProfile::default(),
            serial_ports: Vec::new(),
            serial_picker: None,
            port_owner: None,
            toolbox: None,
            profile_kind: ProfileKind::Ssh,
            editing_profile: None,
            editing_serial: None,
            new_group: String::new(),
            search_open: false,
            search: String::new(),
            search_focus: false,
            search_hits: vec![],
            search_index: 0,
            toast: None,
            rates: RateMeter::default(),
            error,
            login: None,
            login_target: None,
            files: None,
            files_open: false,
            split_chooser: None,
            screenshot,
            started: std::time::Instant::now(),
            screenshot_requested: false,
            sidebar_pinned: false,
            sidebar_reveal: false,
            sidebar_panel_rect: None,
            confirm_exit: false,
            exit_confirmed: false,
            update_status: Arc::new(Mutex::new(update::Status::Checking)),
            update_open: false,
            window_title: String::new(),
        };
        if app.settings.restore_tabs && app.screenshot.is_none() {
            app.restore();
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
        self.respawn(kind, None, ctx)
    }
    /// The pane with this id, if it still exists.
    fn locate(&self, tab_id: u64, pane_id: u64) -> Option<(usize, usize)> {
        let tab = self.tabs.iter().position(|t| t.id == tab_id)?;
        let pane = self.tabs[tab].panes.iter().position(|p| p.id == pane_id)?;
        Some((tab, pane))
    }
    fn terminal_of(&self, tab_id: u64, pane_id: u64) -> Option<Arc<Mutex<Terminal>>> {
        let (tab, pane) = self.locate(tab_id, pane_id)?;
        Some(self.tabs[tab].panes[pane].session.terminal.clone())
    }
    /// The profile and `user@host:port` of the focused pane, when it is a live
    /// SSH session — which is the only thing the server toolbox can act on.
    fn focused_ssh(&self) -> Option<(RemoteProfile, String)> {
        let tab = self.tabs.get(self.active)?;
        let session = &tab.panes.get(tab.focused)?.session;
        if session.remote.is_none() || session.link() != SessionStatus::Live {
            return None;
        }
        match &session.kind {
            SessionKind::Ssh(profile) => Some((
                profile.clone(),
                format!("{}:{}", profile.destination(), profile.port),
            )),
            _ => None,
        }
    }
    /// The link state of the focused pane, which is what the menu's
    /// disconnect / reconnect entry is chosen from.
    fn active_link(&self) -> Option<SessionStatus> {
        let tab = self.tabs.get(self.active)?;
        Some(tab.panes.get(tab.focused)?.session.link())
    }
    /// Whether the focused pane is a remote (SSH / SFTP) session. A live local
    /// shell can still be restarted, so the reconnect entry stays for it.
    fn active_is_remote(&self) -> bool {
        let Some(tab) = self.tabs.get(self.active) else {
            return false;
        };
        tab.panes
            .get(tab.focused)
            .is_some_and(|pane| pane.session.remote.is_some())
    }
    /// Whether any session is still running or connecting, which is what the
    /// exit confirmation protects.
    fn has_live_sessions(&self) -> bool {
        self.tabs.iter().any(|tab| {
            tab.panes
                .iter()
                .any(|pane| pane.session.pending() || pane.session.link() == SessionStatus::Live)
        })
    }
    /// Puts a tab on screen before its connection is attempted, so the
    /// connection's own notices and failures have a console that clearly
    /// belongs to this session.
    fn open_connecting_tab(&mut self, kind: SessionKind) -> (u64, u64) {
        self.next_id += 1;
        let pane = Pane::new(
            self.next_id,
            Session::connecting(kind, self.settings.scrollback),
        );
        let ids = (pane.id, pane.id);
        self.tabs.push(Tab {
            id: pane.id,
            panes: vec![pane],
            focused: 0,
            layout: PaneLayout::Leaf(0),
            seen_output: 0,
        });
        self.active = self.tabs.len() - 1;
        ids
    }
    /// States what is about to be connected to, after blank lines that keep it
    /// apart from the previous run of output. A screen with nothing on it is
    /// left alone, so a brand new pane does not start with empty lines.
    fn announce(&self, tab_id: u64, pane_id: u64, target: Option<&str>) {
        let Some(terminal) = self.terminal_of(tab_id, pane_id) else {
            return;
        };
        let mut terminal = terminal.lock().unwrap_or_else(|e| e.into_inner());
        if !terminal.parser.screen().contents().trim().is_empty() {
            terminal.separator();
        }
        if let Some(target) = target {
            terminal.note(&format!("connect to {target}"));
        }
    }
    /// Shows a short message in the status bar.
    fn notify(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), std::time::Instant::now()));
    }
    /// Whether any pane on screen is still waiting for its connection.
    fn connecting(&self) -> bool {
        self.tabs
            .iter()
            .any(|t| t.panes.iter().any(|p| p.session.pending()))
    }
    /// Feeds the focused pane's byte counters into the rate meter, returning
    /// whether the numbers still need frames to settle.
    fn sample_rates(&mut self) -> bool {
        use std::sync::atomic::Ordering;
        let Some(pane) = self
            .tabs
            .get(self.active)
            .and_then(|t| t.panes.get(t.focused))
        else {
            self.rates.clear();
            return false;
        };
        if pane.session.pending() {
            self.rates.clear();
            return false;
        }
        let up = pane.session.traffic.up.load(Ordering::Relaxed);
        let down = pane.session.traffic.down.load(Ordering::Relaxed);
        self.rates.sample(pane.id, up, down)
    }
    /// Writes a red line on a pane's screen: a cancelled or failed step.
    fn note_error(&self, tab_id: u64, pane_id: u64, line: &str) {
        if let Some(terminal) = self.terminal_of(tab_id, pane_id) {
            terminal
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .note_error(line);
        }
    }
    /// Starts a session, optionally inheriting the screen of the one it
    /// replaces so a reconnect keeps the previous output.
    fn respawn(
        &mut self,
        kind: SessionKind,
        previous: Option<Arc<Mutex<Terminal>>>,
        ctx: &egui::Context,
    ) -> Option<Pane> {
        match Session::spawn_reusing(
            kind,
            self.settings.scrollback,
            remote_ui::wake(ctx),
            previous.clone(),
        ) {
            Ok(s) => {
                self.next_id += 1;
                Some(Pane::new(self.next_id, s))
            }
            Err(e) => {
                let message = format!("{e:#}");
                match &previous {
                    // The failure belongs to the console of the pane it is
                    // about, not to a status line shared by every session.
                    Some(terminal) => terminal
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .note_error(&format!("[错误] connect failed: {message}")),
                    None => self.error = Some(message),
                }
                None
            }
        }
    }
    /// Connects into a pane that already exists, writing the outcome into that
    /// pane's own console.
    fn connect_pane(&mut self, tab_id: u64, pane_id: u64, kind: SessionKind, ctx: &egui::Context) {
        let previous = self.terminal_of(tab_id, pane_id);
        let target = connect_target(&kind);
        self.announce(tab_id, pane_id, target.as_deref());
        match self.respawn(kind.clone(), previous, ctx) {
            Some(pane) => {
                if let Some((tab, index)) = self.locate(tab_id, pane_id) {
                    self.tabs[tab].panes[index] = pane;
                    self.tabs[tab].focused = index;
                    // A shell starts a program rather than connecting, so there
                    // is nothing to call connected.
                    if target.is_some() {
                        self.tabs[tab].panes[index].session.note("connected");
                    }
                    self.active = tab;
                }
            }
            None => {
                // The failure is already in the console; leave a pane that
                // offers a retry instead of a dead one. A serial port that
                // would not open is very often another program holding it, so
                // the owner prompt comes up with the port already filled in.
                if let SessionKind::Serial(profile) = &kind {
                    self.port_owner = Some(PortOwnerWindow::new(profile.port.clone(), ctx));
                }
                if let Some((tab, index)) = self.locate(tab_id, pane_id) {
                    let terminal = self.tabs[tab].panes[index].session.terminal.clone();
                    self.tabs[tab].panes[index] = Pane::new(
                        pane_id,
                        Session::disconnected_reusing(
                            kind,
                            self.settings.scrollback,
                            Some(terminal),
                        ),
                    );
                    self.tabs[tab].focused = index;
                    self.active = tab;
                }
            }
        }
    }
    fn add_connection(&mut self, connection: Arc<Connection>, ctx: &egui::Context) {
        // A reconnect reuses the pane's screen, so the output seen before the
        // drop stays in the scrollback. `login_target` names the pane the new
        // session is about to replace.
        let previous = self
            .login_target
            .and_then(|(tab_id, pane_id)| self.terminal_of(tab_id, pane_id));
        self.next_id += 1;
        let pane = Pane::new(
            self.next_id,
            Session::from_remote(
                connection,
                self.settings.scrollback,
                remote_ui::wake(ctx),
                previous,
            ),
        );
        if let Some((tab_id, pane_id)) = self.login_target.take()
            && let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id)
            && let Some(index) = t.panes.iter().position(|p| p.id == pane_id)
        {
            t.panes[index] = pane;
            t.focused = index;
            t.panes[index].session.note("connected");
            return;
        }
        self.tabs.push(Tab {
            id: pane.id,
            panes: vec![pane],
            focused: 0,
            layout: PaneLayout::Leaf(0),
            seen_output: 0,
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
        // A test must never write the real settings file. An app test that adds
        // or removes a connection calls this, and without the guard it replaced
        // the user's saved profiles with the test's own defaults — which is
        // exactly how connections were lost. The guard is what keeps that from
        // happening again, so it stays even though tests do not check the file.
        #[cfg(not(test))]
        {
            if let Err(e) = self.settings.save() {
                self.error = Some(format!("{e:#}"));
            }
        }
    }
    /// Rebuilds the saved tabs. Every pane, local included, is restored
    /// disconnected, so no shell starts and no host is contacted until the user
    /// asks for it.
    fn restore(&mut self) {
        for saved in self.settings.workspace.clone().into_iter().take(32) {
            if saved.sessions.is_empty()
                || saved.sessions.len() > 32
                || !saved.layout.valid(saved.sessions.len())
            {
                continue;
            }
            let mut panes = vec![];
            for kind in saved.sessions {
                self.next_id += 1;
                panes.push(Pane::new(
                    self.next_id,
                    Session::disconnected(kind, self.settings.scrollback),
                ));
            }
            if !panes.is_empty() && saved.layout.valid(panes.len()) {
                self.tabs.push(Tab {
                    id: panes[0].id,
                    focused: saved.focused.min(panes.len() - 1),
                    panes,
                    layout: saved.layout,
                    seen_output: 0,
                });
            }
        }
    }
    fn execute(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::NewWindow => match std::env::current_exe() {
                Ok(exe) => {
                    if let Err(error) = std::process::Command::new(exe).spawn() {
                        self.error = Some(format!("无法新建窗口：{error}"));
                    }
                }
                Err(error) => self.error = Some(format!("无法定位程序：{error}")),
            },
            Action::DuplicateSession(i) => {
                if let Some(tab) = self.tabs.get(i) {
                    let kind = tab.panes[tab.focused].session.kind.clone();
                    self.execute(Action::New(kind), ctx);
                }
            }
            Action::New(kind) => {
                // The tab exists before the connection does, so the connection's
                // notices and failures land in its own console.
                let (tab_id, pane_id) = self.open_connecting_tab(kind.clone());
                match kind {
                    SessionKind::Ssh(p) | SessionKind::Sftp(p) => {
                        self.login_target = Some((tab_id, pane_id));
                        self.announce(
                            tab_id,
                            pane_id,
                            Some(&format!("{}:{}", p.destination(), p.port)),
                        );
                        let console = self.terminal_of(tab_id, pane_id);
                        self.login = Some(Login::new(p, console));
                    }
                    kind => self.connect_pane(tab_id, pane_id, kind, ctx),
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
                            Session::from_remote(
                                c,
                                self.settings.scrollback,
                                remote_ui::wake(ctx),
                                None,
                            ),
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
            Action::CloseOtherTabs(keep) => {
                if keep < self.tabs.len() {
                    let kept = self.tabs.remove(keep);
                    self.tabs.clear();
                    self.tabs.push(kept);
                    self.active = 0;
                }
            }
            Action::CloseDisconnectedTabs => {
                self.tabs
                    .retain(|tab| tab_link(tab) != SessionStatus::Detached);
                self.active = self.active.min(self.tabs.len().saturating_sub(1));
            }
            Action::CheckUpdates => self.check_updates(ctx),
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
                    let tab_id = t.id;
                    let kind = t.panes[t.focused].session.kind.clone();
                    let pane_id = t.panes[t.focused].id;
                    // The screen outlives the session so the replacement can
                    // carry the scrollback over; the Arc keeps it alive while
                    // the old session is dropped.
                    let previous = Some(t.panes[t.focused].session.terminal.clone());
                    if let SessionKind::Ssh(p) | SessionKind::Sftp(p) = kind {
                        self.login_target = Some((tab_id, pane_id));
                        self.announce(
                            tab_id,
                            pane_id,
                            Some(&format!("{}:{}", p.destination(), p.port)),
                        );
                        let console = self.terminal_of(tab_id, pane_id);
                        self.login = Some(Login::new(p, console));
                    } else {
                        // Drop the old session before starting the replacement:
                        // a serial device is exclusive, and a shell is killed
                        // so its leftover output cannot land in the screen that
                        // is about to be reused.
                        let index = self.tabs[self.active].focused;
                        self.tabs[self.active].panes[index] = Pane::new(
                            pane_id,
                            Session::disconnected_reusing(
                                kind.clone(),
                                self.settings.scrollback,
                                previous,
                            ),
                        );
                        self.connect_pane(tab_id, pane_id, kind, ctx);
                    }
                }
            }
            Action::Disconnect => {
                if let Some(tab) = self.tabs.get(self.active) {
                    let pane = &tab.panes[tab.focused];
                    let pending = pane.session.pending();
                    let kind = pane.session.kind.clone();
                    let pane_id = pane.id;
                    let terminal = pane.session.terminal.clone();
                    // A pane that has not connected yet is already "disconnected";
                    // a live one is dropped while its screen is kept, so the
                    // output stays readable and Alt+R can bring it back.
                    if !pending {
                        let index = self.tabs[self.active].focused;
                        // Red, like the other end-of-session notices: leaving a
                        // session is worth noticing, and the reconnect hint is
                        // the actionable part.
                        self.tabs[self.active].panes[index]
                            .session
                            .note_error(&format!("已断开（{} 重连）", alt_accel("R")));
                        self.tabs[self.active].panes[index] = Pane::new(
                            pane_id,
                            Session::disconnected_reusing(
                                kind,
                                self.settings.scrollback,
                                Some(terminal),
                            ),
                        );
                    }
                }
            }
            Action::Remote => {
                self.remote = RemoteProfile::default();
                self.serial = SerialProfile::default();
                self.serial_ports = serial::available_ports();
                self.profile_kind = ProfileKind::Ssh;
                self.editing_profile = None;
                self.editing_serial = None;
                self.remote_open = true;
            }
            Action::Edit(i) => {
                self.remote = self.settings.profiles[i].clone();
                self.profile_kind = ProfileKind::Ssh;
                self.editing_profile = Some(i);
                self.editing_serial = None;
                self.remote_open = true;
            }
            Action::EditSerial(i) => {
                self.serial = self.settings.serial_profiles[i].clone();
                self.serial_ports = serial::available_ports();
                self.profile_kind = ProfileKind::Serial;
                self.editing_serial = Some(i);
                self.editing_profile = None;
                self.remote_open = true;
            }
            Action::Remove(i) => {
                self.settings.profiles.remove(i);
                self.persist();
            }
            Action::RemoveSerial(i) => {
                self.settings.serial_profiles.remove(i);
                self.persist();
            }
            Action::Duplicate(i) => {
                if let Some(profile) = self.settings.profiles.get(i).cloned() {
                    let taken: Vec<String> = self
                        .settings
                        .profiles
                        .iter()
                        .map(|profile| profile.name.clone())
                        .collect();
                    let base = if profile.name.trim().is_empty() {
                        profile.label()
                    } else {
                        profile.name.clone()
                    };
                    let mut copy = profile;
                    copy.name = unique_copy_name(&base, &taken);
                    self.settings.profiles.push(copy);
                    self.persist();
                }
            }
            Action::DuplicateSerial(i) => {
                if let Some(profile) = self.settings.serial_profiles.get(i).cloned() {
                    let taken: Vec<String> = self
                        .settings
                        .serial_profiles
                        .iter()
                        .map(|profile| profile.name.clone())
                        .collect();
                    let base = if profile.name.trim().is_empty() {
                        profile.label()
                    } else {
                        profile.name.clone()
                    };
                    let mut copy = profile;
                    copy.name = unique_copy_name(&base, &taken);
                    self.settings.serial_profiles.push(copy);
                    self.persist();
                }
            }
            Action::AdoptSshConfig(profile) => match profile.validate() {
                Ok(()) => {
                    let label = profile.label();
                    self.settings.profiles.push(profile);
                    self.persist();
                    self.notify(format!("已保存连接 {label}"));
                }
                Err(e) => self.error = Some(e.to_string()),
            },
            Action::SerialPicker => self.serial_picker = Some(SerialPicker::new()),
            Action::FindPortOwner(port) => {
                self.port_owner = Some(PortOwnerWindow::new(port, ctx));
            }
            Action::Toolbox => match self.focused_ssh() {
                Some((profile, _)) => self.toolbox = Some(crate::toolbox::Toolbox::new(&profile)),
                None => self.error = Some("服务器工具箱仅用于已连接的 SSH 会话".into()),
            },
            Action::Files => {
                self.ensure_files(ctx);
            }
        }
        self.search_hits.clear();
    }
    fn shortcuts(&mut self, ctx: &egui::Context) -> Option<Action> {
        if self.settings_open
            || self.remote_open
            || self.groups_open
            || self.help_open
            || self.update_open
            || self.login.is_some()
            || self.files_open
        {
            return None;
        }
        let mut action = None;
        ctx.input_mut(|i| {
            // A handled shortcut also produces a text event — egui-winit only
            // filters those out for Ctrl, not for Alt, so Alt+R arrives as a key
            // *and* as `Text("r")`. Left alone, that text would be typed into the
            // terminal (and, on an ended session, reported as a write error).
            let mut handled = false;
            i.events.retain(|e| {
                let keep = (|| {
                    if let egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } = e
                    {
                        if modifiers.command && modifiers.shift {
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
                        // Cmd+Tab is the system app switcher on macOS, so the
                        // tab shortcut stays on Ctrl there and accepts either.
                        if (modifiers.ctrl || modifiers.command) && *key == Key::Tab {
                            if !self.tabs.is_empty() {
                                self.active = (self.active + 1) % self.tabs.len();
                            }
                            return false;
                        }
                        if modifiers.command && *key == Key::Comma {
                            self.settings_open = true;
                            return false;
                        }
                        if modifiers.alt && *key == Key::ArrowRight {
                            if let Some(t) = self.tabs.get_mut(self.active) {
                                t.focused = (t.focused + 1) % t.panes.len();
                            }
                            return false;
                        }
                        // Disconnect only applies while something is connected;
                        // a live remote connection cannot be "reconnected",
                        // only a local shell can be restarted from here.
                        if modifiers.alt && *key == Key::C {
                            if self.active_link() == Some(SessionStatus::Live) {
                                action = Some(Action::Disconnect);
                            }
                            return false;
                        }
                        if modifiers.alt && *key == Key::R {
                            let live = self.active_link() == Some(SessionStatus::Live);
                            if !(live && self.active_is_remote()) {
                                action = Some(Action::Restart);
                            }
                            return false;
                        }
                        if modifiers.command
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
                })();
                if !keep {
                    handled = true;
                }
                keep
            });
            if handled {
                i.events.retain(|e| !matches!(e, egui::Event::Text(_)));
            }
        });
        action
    }

    /// The one way out: close now, or ask first while sessions are still live
    /// and the setting wants it. Every exit path 鈥?the titlebar button, the
    /// platform's close request, the dialog's own button 鈥?comes through here.
    pub(super) fn request_exit(&mut self, ctx: &egui::Context) {
        if self.exit_confirmed || !self.settings.confirm_on_exit || !self.has_live_sessions() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            self.confirm_exit = true;
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
        // While a candidate list is open the Enter belongs to the IME, not to
        // the widget underneath. Letting it through made a text field surrender
        // focus mid-composition, which threw away the characters being composed;
        // the commit itself arrives as its own event and is untouched.
        if editing::ime_composing(ctx) {
            ctx.input_mut(|input| editing::swallow_ime_keys(&mut input.events));
        }
        self.handle_dropped_files(ctx);
        // A platform close (Alt+F4, taskbar, the macOS traffic light) is held
        // back once while sessions are still connected: the platform is told to
        // abort it, and the dialog decides the rest. Without the confirmation
        // the request is answered with a close of our own, because the native
        // integration leaves the decision to the app.
        if self.screenshot.is_none()
            && !self.exit_confirmed
            && ctx.input(|i| i.viewport().close_requested())
        {
            if self.settings.confirm_on_exit && self.has_live_sessions() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.confirm_exit = true;
            } else {
                self.request_exit(ctx);
            }
        }
        self.exit_confirm(ctx);
        // macOS keeps its native resize border; the other platforms are
        // frameless and have to detect the edges themselves.
        if !cfg!(target_os = "macos") {
            self.resize_grips(ctx);
        }
        self.tick_status(ctx);
        let mut action = self.shortcuts(ctx);
        self.topbar(ctx, &mut action);
        self.mark_active_seen();
        self.update_window_title(ctx);
        self.drop_overlay(ctx, p);
        self.status_bar(ctx, &mut action, p);
        if self.settings.sidebar {
            if self.settings.auto_hide_sidebar && !self.sidebar_pinned {
                self.sidebar_overlay(ctx, &mut action);
            } else {
                self.sidebar(ctx, &mut action);
            }
        }
        self.split_chooser(ctx, p);
        self.poll_zmodem_offer(ctx);
        if let Some(text) = self.panes(ctx, &mut action, p) {
            self.notify(text);
        }
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
        // Last, so every Enter check above saw the composition state as it was
        // when the frame started rather than after this frame's commit.
        editing::track_ime(ctx);
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
/// What a connection points at, for its console notice. A local shell starts a
/// program rather than connecting to anything, so it has no target.
fn connect_target(kind: &SessionKind) -> Option<String> {
    match kind {
        SessionKind::Serial(profile) => {
            Some(format!("{} @ {} bps", profile.port.trim(), profile.baud))
        }
        SessionKind::Ssh(profile) | SessionKind::Sftp(profile) => {
            Some(format!("{}:{}", profile.destination(), profile.port))
        }
        SessionKind::Local(_) => None,
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

/// A name for a duplicated connection: `<base> 副本`, numbered when that name
/// is already in use.
fn unique_copy_name(base: &str, taken: &[String]) -> String {
    let candidate = format!("{base} 副本");
    if !taken.iter().any(|name| name == &candidate) {
        return candidate;
    }
    for index in 2.. {
        let candidate = format!("{base} 副本 {index}");
        if !taken.iter().any(|name| name == &candidate) {
            return candidate;
        }
    }
    unreachable!()
}

#[cfg(test)]
mod helper_tests;
#[cfg(all(test, windows))]
mod tests;
