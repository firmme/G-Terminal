use crate::{
    editing, icons,
    remote_ui::{self, Files, Login, format_size, hint},
    theme::{Palette, load_fonts},
    view::Pane,
};
use eframe::egui::{self, Align, Key, Layout, Rect, RichText, Sense};
use g_terminal::{
    config::{
        BAUD_RATES, DEFAULT_BAUD, Forward, RemoteProfile, SEARCH_ENGINES, SerialProfile, Settings,
        search_engine_label,
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
    Split(Axis),
    CloseTab(usize),
    ClosePane,
    Restart,
    Disconnect,
    Remote,
    Edit(usize),
    Remove(usize),
    SerialPicker,
    EditSerial(usize),
    RemoveSerial(usize),
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
        if self.ports.is_empty() {
            self.port = "auto".into();
        } else if !self.ports.iter().any(|p| p.name == self.port) {
            self.port = self.ports[0].name.clone();
        }
    }
}

/// How long a status-bar message stays on screen.
const TOAST_LIFETIME: std::time::Duration = std::time::Duration::from_millis(2500);

/// Turns a pane's byte counters into a smoothed per-second rate. The counters
/// are cumulative, so a rate is the difference since the last sample.
#[derive(Default)]
struct RateMeter {
    pane: Option<u64>,
    at: Option<std::time::Instant>,
    up: u64,
    down: u64,
    up_per_sec: f64,
    down_per_sec: f64,
}

/// Traffic this quiet counts as idle, so the arrows settle back to grey.
const TRAFFIC_IDLE: f64 = 1.0;

impl RateMeter {
    /// Samples a pane, returning whether the rates are still worth animating.
    ///
    /// Every frame samples, rather than waiting for a fixed interval: a short
    /// burst has to light the arrows on the frames it causes, and those are the
    /// only frames guaranteed to happen.
    fn sample(&mut self, pane: u64, up: u64, down: u64) -> bool {
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

    fn clear(&mut self) {
        let pane = self.pane;
        *self = Self {
            pane,
            ..Self::default()
        };
    }
}

/// Bytes per second as an adaptive bit rate, which is how a link is described.
fn format_bitrate(bytes_per_sec: f64) -> String {
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
    fn note(&self, tab_id: u64, pane_id: u64, line: &str) {
        if let Some(terminal) = self.terminal_of(tab_id, pane_id) {
            terminal
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .note(line);
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
                // offers a retry instead of a dead one.
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
                    seen_output: 0,
                });
            }
        }
    }
    fn execute(&mut self, action: Action, ctx: &egui::Context) {
        match action {
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
                        self.tabs[self.active].panes[index]
                            .session
                            .note("已断开（Alt+R 重连）");
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
            Action::SerialPicker => self.serial_picker = Some(SerialPicker::new()),
            Action::Toolbox => match self.focused_ssh() {
                Some((profile, _)) => self.toolbox = Some(crate::toolbox::Toolbox::new(&profile)),
                None => self.error = Some("服务器工具箱仅用于已连接的 SSH 会话".into()),
            },
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
                        if modifiers.alt && *key == Key::C {
                            action = Some(Action::Disconnect);
                            return false;
                        }
                        if modifiers.alt && *key == Key::R {
                            action = Some(Action::Restart);
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
                        ui.menu_button(RichText::new("G  菜单").color(p.accent), |ui| {
                            for (icon, text, shortcut, a) in [
                                (
                                    icons::Icon::Terminal,
                                    "新建终端",
                                    Some(accel("Ctrl+Shift+T")),
                                    Action::New(SessionKind::Local(
                                        self.settings.default_shell.clone(),
                                    )),
                                ),
                                (icons::Icon::Host, "新建 SSH 连接", None, Action::Remote),
                                (
                                    icons::Icon::Terminal,
                                    "连接串口",
                                    None,
                                    Action::SerialPicker,
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
                                (
                                    icons::Icon::Restart,
                                    "重新连接 / 重启",
                                    None,
                                    Action::Restart,
                                ),
                            ] {
                                if icons::icon_row(ui, icon, text, shortcut.as_deref(), p).clicked()
                                {
                                    *action = Some(a);
                                    ui.close();
                                }
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
                            if icons::icon_row(
                                ui,
                                icons::Icon::Settings,
                                "服务器工具箱（初始化服务器）",
                                None,
                                p,
                            )
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
                                                egui::Frame::new()
                                                    .fill(if i == self.active {
                                                        p.raised
                                                    } else if hovered {
                                                        p.accent.gamma_multiply(0.18)
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
                            for shell in local_shells() {
                                // Double-click, like every other row in the
                                // sidebar, so a stray click cannot open a pane.
                                let row = icons::icon_row(
                                    ui,
                                    icons::Icon::Terminal,
                                    &shell.label,
                                    None,
                                    p,
                                );
                                if row.double_clicked() {
                                    *action =
                                        Some(Action::New(SessionKind::Local(shell.value.clone())));
                                }
                                // The executable path matters on Unix, where
                                // several shells can share a name.
                                row.on_hover_text(if shell.value.contains('/') {
                                    format!("双击打开 · {}", shell.value)
                                } else {
                                    "双击打开".to_string()
                                });
                            }
                            ui.separator();
                            let serial =
                                icons::icon_row(ui, icons::Icon::Host, "连接串口…", None, p);
                            if serial.double_clicked() {
                                *action = Some(Action::SerialPicker);
                            }
                            serial.on_hover_text("双击打开串口选择");
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
                    let any_profiles = !self.settings.profiles.is_empty()
                        || !self.settings.serial_profiles.is_empty();
                    for group in groups {
                        let count = self
                            .settings
                            .profiles
                            .iter()
                            .filter(|p| p.group == group)
                            .count()
                            + self
                                .settings
                                .serial_profiles
                                .iter()
                                .filter(|p| p.group == group)
                                .count();
                        if group.is_empty() && count == 0 && any_profiles {
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
                            for (index, profile) in self
                                .settings
                                .serial_profiles
                                .iter()
                                .enumerate()
                                .filter(|(_, p)| p.group == group)
                            {
                                let r = icons::icon_row(
                                    ui,
                                    icons::Icon::Terminal,
                                    &profile.label(),
                                    None,
                                    p,
                                );
                                if r.double_clicked() {
                                    *action =
                                        Some(Action::New(SessionKind::Serial(profile.clone())));
                                }
                                r.on_hover_text(format!(
                                    "串口 {} · {} bps · 双击连接 / 右键管理",
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
                        .selected_text(shell_label(&self.settings.default_shell))
                        .show_ui(ui, |ui| {
                            for shell in local_shells() {
                                changed |= ui
                                    .selectable_value(
                                        &mut self.settings.default_shell,
                                        shell.value.clone(),
                                        &shell.label,
                                    )
                                    .changed();
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.label("浏览器搜索");
                    egui::ComboBox::from_id_salt("search-engine")
                        .selected_text(search_engine_label(&self.settings.search_engine))
                        .show_ui(ui, |ui| {
                            for (key, label, _) in SEARCH_ENGINES {
                                changed |= ui
                                    .selectable_value(
                                        &mut self.settings.search_engine,
                                        key.to_string(),
                                        label,
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
        let mut refresh_ports = false;
        egui::Window::new("连接配置")
            .open(&mut open)
            .collapsible(false)
            .default_width(480.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("类型");
                    ui.selectable_value(&mut self.profile_kind, ProfileKind::Ssh, "SSH");
                    ui.selectable_value(&mut self.profile_kind, ProfileKind::Serial, "串口");
                });
                ui.separator();
                match self.profile_kind {
                    ProfileKind::Ssh => {
                        egui::Grid::new("connection-form")
                            .spacing([12.0, 6.0])
                            .min_col_width(72.0)
                            .show(ui, |ui| {
                                ui.label("主机 / IP");
                                editing::field_with(ui, &mut self.remote.host, |edit| {
                                    edit.desired_width(280.0)
                                });
                                ui.end_row();
                                ui.label("端口");
                                ui.add(
                                    egui::DragValue::new(&mut self.remote.port).range(1..=65535),
                                );
                                ui.end_row();
                                ui.label("连接名称");
                                editing::field_with(ui, &mut self.remote.name, |edit| {
                                    edit.desired_width(280.0)
                                });
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
                                editing::field_with(ui, &mut self.remote.user, |edit| {
                                    edit.desired_width(280.0)
                                });
                                ui.end_row();
                                ui.label("私钥路径");
                                editing::field_with(ui, &mut self.remote.identity, |edit| {
                                    edit.desired_width(280.0)
                                });
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
                                        editing::field_with(ui, &mut j.host, |edit| {
                                            edit.desired_width(280.0)
                                        });
                                        ui.end_row();
                                        ui.label("端口");
                                        ui.add(egui::DragValue::new(&mut j.port).range(1..=65535));
                                        ui.end_row();
                                        ui.label("用户名");
                                        editing::field_with(ui, &mut j.user, |edit| {
                                            edit.desired_width(280.0)
                                        });
                                        ui.end_row();
                                        ui.label("私钥");
                                        editing::field_with(ui, &mut j.identity, |edit| {
                                            edit.desired_width(280.0)
                                        });
                                        ui.end_row();
                                    });
                            }
                        });
                        egui::CollapsingHeader::new("本地端口转发（监听 127.0.0.1）").show(
                            ui,
                            |ui| {
                                let mut remove = None;
                                for (i, f) in self.remote.forwards.iter_mut().enumerate() {
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            egui::DragValue::new(&mut f.bind_port).range(1..=65535),
                                        );
                                        ui.label("→");
                                        editing::field_with(ui, &mut f.target_host, |edit| {
                                            edit.desired_width(160.0)
                                        });
                                        ui.add(
                                            egui::DragValue::new(&mut f.target_port)
                                                .range(1..=65535),
                                        );
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
                            },
                        );
                        ui.label(hint("认证在连接时进行，密码不写入配置。", p));
                    }
                    ProfileKind::Serial => {
                        egui::Grid::new("serial-form")
                            .spacing([12.0, 6.0])
                            .min_col_width(72.0)
                            .show(ui, |ui| {
                                ui.label("串口");
                                ui.horizontal(|ui| {
                                    editing::field_with(ui, &mut self.serial.port, |edit| {
                                        edit.desired_width(180.0)
                                            .hint_text(format!("{} 或 auto", serial::PORT_EXAMPLE))
                                    });
                                    egui::ComboBox::from_id_salt("serial-port-pick")
                                        .selected_text("▾")
                                        .width(150.0)
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut self.serial.port,
                                                "auto".to_string(),
                                                "auto（自动）",
                                            );
                                            for port in &self.serial_ports {
                                                let label = if port.bluetooth {
                                                    format!("{}（蓝牙）", port.name)
                                                } else {
                                                    port.name.clone()
                                                };
                                                ui.selectable_value(
                                                    &mut self.serial.port,
                                                    port.name.clone(),
                                                    label,
                                                );
                                            }
                                            if self.serial_ports.is_empty() {
                                                ui.label(hint("未检测到串口", p));
                                            }
                                        });
                                    if ui.small_button("刷新").clicked() {
                                        refresh_ports = true;
                                    }
                                });
                                ui.end_row();
                                ui.label("波特率");
                                egui::ComboBox::from_id_salt("serial-baud")
                                    .width(120.0)
                                    .selected_text(self.serial.baud.to_string())
                                    .show_ui(ui, |ui| {
                                        for baud in BAUD_RATES {
                                            ui.selectable_value(
                                                &mut self.serial.baud,
                                                baud,
                                                baud.to_string(),
                                            );
                                        }
                                    });
                                ui.end_row();
                                ui.label("连接名称");
                                editing::field_with(ui, &mut self.serial.name, |edit| {
                                    edit.desired_width(280.0)
                                });
                                ui.end_row();
                                ui.label("分组");
                                egui::ComboBox::from_id_salt("serial-group")
                                    .width(200.0)
                                    .selected_text(if self.serial.group.is_empty() {
                                        "未分组"
                                    } else {
                                        &self.serial.group
                                    })
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut self.serial.group,
                                            String::new(),
                                            "未分组",
                                        );
                                        for group in &self.settings.groups {
                                            ui.selectable_value(
                                                &mut self.serial.group,
                                                group.clone(),
                                                group,
                                            );
                                        }
                                    });
                                ui.end_row();
                            });
                        ui.label(hint(
                            "串口填 auto 时，连接优先选普通串口，并跳过本程序已占用的串口。",
                            p,
                        ));
                    }
                }
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
        if refresh_ports {
            self.serial_ports = serial::available_ports();
        }
        if save {
            let saved = match self.profile_kind {
                ProfileKind::Ssh => self.remote.validate().map(|()| {
                    if let Some(i) = self.editing_profile {
                        self.settings.profiles[i] = self.remote.clone();
                    } else {
                        self.settings.profiles.push(self.remote.clone());
                    }
                }),
                ProfileKind::Serial => self.serial.validate().map(|()| {
                    if let Some(i) = self.editing_serial {
                        self.settings.serial_profiles[i] = self.serial.clone();
                    } else {
                        self.settings.serial_profiles.push(self.serial.clone());
                    }
                }),
            };
            match saved {
                Ok(()) => {
                    self.remote_open = false;
                    self.persist();
                    if connect {
                        *action = Some(match self.profile_kind {
                            ProfileKind::Ssh => Action::New(SessionKind::Ssh(self.remote.clone())),
                            ProfileKind::Serial => {
                                Action::New(SessionKind::Serial(self.serial.clone()))
                            }
                        });
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
                    editing::field_with(ui, &mut self.new_group, |edit| edit.desired_width(180.0));
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
                    for p in &mut self.settings.serial_profiles {
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
                    for p in &mut self.settings.serial_profiles {
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
        if let Some(mut toolbox) = self.toolbox.take() {
            // The script is typed into the focused session, so the target is
            // resolved once, right before it runs.
            let target = self.focused_ssh().map(|(_, target)| target);
            let mut open = true;
            let script = match &target {
                Some(target) => toolbox.show(ctx, &mut open, p, target),
                None => {
                    self.error = Some("会话已断开，服务器工具箱不可用".into());
                    None
                }
            };
            if let Some(script) = script {
                open = false;
                let count = script.chars().count();
                let sent = self
                    .tabs
                    .get_mut(self.active)
                    .and_then(|t| t.panes.get_mut(t.focused))
                    .map(|pane| pane.session.write(script.into_bytes()));
                match sent {
                    Some(Ok(())) => self.notify(format!("已发送服务器初始化脚本（{count} 字符）")),
                    Some(Err(e)) => self.error = Some(format!("{e:#}")),
                    None => {}
                }
            }
            if open && target.is_some() {
                self.toolbox = Some(toolbox);
            }
        }
        if self.serial_picker.is_some() {
            let mut open = true;
            let mut connect = false;
            let mut cancel = false;
            let mut refresh = false;
            if let Some(picker) = &mut self.serial_picker {
                egui::Window::new("连接串口")
                    .open(&mut open)
                    .collapsible(false)
                    .resizable(false)
                    .default_width(340.0)
                    .show(ctx, |ui| {
                        if picker.ports.is_empty() {
                            ui.label(hint("未检测到串口设备。可在连接配置里填 auto。", p));
                        } else {
                            ui.label(hint("普通串口在前，蓝牙串口在后。", p));
                            egui::ScrollArea::vertical()
                                .max_height(220.0)
                                .show(ui, |ui| {
                                    for port in &picker.ports {
                                        let label = if port.bluetooth {
                                            format!("{}（蓝牙）", port.name)
                                        } else {
                                            port.name.clone()
                                        };
                                        ui.radio_value(&mut picker.port, port.name.clone(), label);
                                    }
                                });
                        }
                        ui.horizontal(|ui| {
                            ui.label("波特率");
                            egui::ComboBox::from_id_salt("picker-baud")
                                .width(120.0)
                                .selected_text(picker.baud.to_string())
                                .show_ui(ui, |ui| {
                                    for baud in BAUD_RATES {
                                        ui.selectable_value(
                                            &mut picker.baud,
                                            baud,
                                            baud.to_string(),
                                        );
                                    }
                                });
                        });
                        ui.horizontal(|ui| {
                            if ui.button("连接").clicked() {
                                connect = true;
                            }
                            if ui.button("刷新").clicked() {
                                refresh = true;
                            }
                            if ui.button("取消").clicked() {
                                cancel = true;
                            }
                        });
                    });
                if refresh {
                    picker.refresh();
                }
            }
            if cancel {
                open = false;
            }
            if connect && let Some(picker) = &self.serial_picker {
                *action = Some(Action::New(SessionKind::Serial(SerialProfile {
                    name: picker.port.clone(),
                    port: picker.port.clone(),
                    baud: picker.baud,
                    group: String::new(),
                })));
                open = false;
            }
            if !open {
                self.serial_picker = None;
            }
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
                // Ctrl+C stays the terminal interrupt on every platform; only
                // the clipboard accelerators follow the Cmd / Ctrl split.
                let copy_paste = if ACCEL == "Cmd" {
                    "Cmd+C / Cmd+V".to_string()
                } else {
                    accel("Ctrl+Shift+C / V")
                };
                for (key, description) in [
                    (accel("Ctrl+Shift+T / W"), "新建标签 / 关闭窗格"),
                    (accel("Ctrl+Shift+D / E"), "左右 / 上下分屏"),
                    ("Ctrl+Tab / Alt+Right".to_string(), "切换标签 / 窗格"),
                    (copy_paste, "复制 / 粘贴"),
                    ("Ctrl+C".to_string(), "终端中断"),
                    ("Alt+C / Alt+R".to_string(), "断开 / 重连当前会话"),
                    (accel("Ctrl+Shift+F"), "全部保留历史查找"),
                    (accel("Ctrl+Shift+B / Ctrl+,"), "导航栏 / 设置"),
                    ("中键 / Shift+鼠标".to_string(), "粘贴 / 强制选择"),
                    (accel("Ctrl+Plus / Minus / 0"), "字号放大 / 缩小 / 重置"),
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
                // Backing out leaves the pre-created pane, marked so it still
                // offers a retry.
                if let Some((tab_id, pane_id)) = self.login_target.take() {
                    self.note(tab_id, pane_id, "connect cancelled");
                    if let Some((tab, index)) = self.locate(tab_id, pane_id) {
                        let kind = self.tabs[tab].panes[index].session.kind.clone();
                        let terminal = self.tabs[tab].panes[index].session.terminal.clone();
                        self.tabs[tab].panes[index] = Pane::new(
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
        // macOS keeps its native resize border; the other platforms are
        // frameless and have to detect the edges themselves.
        if !cfg!(target_os = "macos") {
            self.resize_grips(ctx);
        }
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
        let down_rate = self.rates.down_per_sec;
        let up_rate = self.rates.up_per_sec;
        let mut action = self.shortcuts(ctx);
        self.topbar(ctx, &mut action);
        // Looking at a tab clears its new-output mark, which is why this lands
        // after the strip has been drawn.
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.seen_output = tab_output(tab);
        }
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
                                action = Some(Action::Files);
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
        // Collected inside the pane loop because the pane borrows the tab list,
        // and applied after it because the toast lives on the app.
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
                            action = Some(Action::New(SessionKind::Local(
                                self.settings.default_shell.clone(),
                            )));
                        }
                    });
                }
            });
        if let Some(text) = notice {
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

/// Which frame icon a titlebar button paints. `Restore` and `Maximize` are the
/// same button in two states.
#[derive(Clone, Copy, PartialEq)]
enum TitleButton {
    Close,
    Maximize,
    Restore,
    Minimize,
}

/// The topbar's inner margin. macOS runs the content under a hidden native
/// titlebar, so the left side has to clear the traffic lights; every other
/// platform owns the whole row.
fn topbar_margin() -> egui::Margin {
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

/// Eight small squares in a 3x3 ring, chasing each other round while a
/// connection is being made.
fn connecting_spinner(ui: &mut egui::Ui, p: Palette) {
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
fn resize_grip(ui: &mut egui::Ui, p: Palette) -> egui::Response {
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
        // `command` is what the accelerators are written against: it is Ctrl
        // on Windows and Cmd on macOS, and egui-winit sets both for Ctrl.
        let mods = Modifiers {
            ctrl: true,
            command: true,
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

    /// The wheel scrolls the pane's own history, and a restart keeps that
    /// history by reusing the same screen. What the reused screen *contains* is
    /// `terminal`'s business and is covered there; here the point is the wiring,
    /// which is why the assertion is on the screen's identity rather than on
    /// output that a dying shell could still race with.
    #[test]
    fn wheel_scrolls_history_and_restart_keeps_it() {
        let ctx = egui::Context::default();
        let mut app = App::from_settings(&ctx, Settings::default(), None, None);
        let size = Vec2::new(1280.0, 800.0);
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
        {
            let mut terminal = app.tabs[0].panes[0].session.terminal.lock().unwrap();
            for i in 0..80 {
                terminal.process(format!("history {i}\r\n").as_bytes());
            }
        }
        let center = egui::Pos2::new(700.0, 400.0);
        frame(
            &mut app,
            &ctx,
            vec![Event::PointerMoved(center)],
            Modifiers::NONE,
            size,
        );
        for _ in 0..20 {
            frame(
                &mut app,
                &ctx,
                vec![Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: Vec2::new(0.0, 1.0),
                    modifiers: Modifiers::NONE,
                }],
                Modifiers::NONE,
                size,
            );
        }
        let scrolled = app.tabs[0].panes[0]
            .session
            .terminal
            .lock()
            .unwrap()
            .parser
            .screen()
            .scrollback();
        assert!(scrolled > 0, "the wheel did not move the history");

        let before = app.tabs[0].panes[0].session.terminal.clone();
        app.execute(Action::Restart, &ctx);
        assert!(app.error.is_none(), "{:?}", app.error);
        let after = app.tabs[0].panes[0].session.terminal.clone();
        assert!(
            Arc::ptr_eq(&before, &after),
            "the restart did not carry the screen over"
        );
    }

    /// The spinner has to actually paint: a bare panel so the only shapes are
    /// the eight squares of the ring.
    #[test]
    fn the_spinner_paints_its_whole_ring() {
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| connecting_spinner(ui, Palette::new(false)));
        });
        assert!(
            output.shapes.len() >= 8,
            "the spinner painted {} shapes",
            output.shapes.len()
        );
    }

    /// The spinner is driven by panes that have no connection yet, and it has
    /// to stop once the attempt settles either way.
    #[test]
    fn the_spinner_runs_only_while_a_connection_is_pending() {
        let ctx = egui::Context::default();
        let mut app = App::from_settings(&ctx, Settings::default(), None, None);
        let size = Vec2::new(1280.0, 800.0);
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
        assert!(!app.connecting());
        let serial = SessionKind::Serial(SerialProfile {
            port: "COM199".into(),
            ..Default::default()
        });
        let (tab_id, pane_id) = app.open_connecting_tab(serial.clone());
        assert!(app.connecting(), "a pending pane must spin");
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
        app.connect_pane(tab_id, pane_id, serial, &ctx);
        assert!(!app.connecting(), "a settled attempt must stop the spinner");
    }

    /// The arrows light on a burst and settle back to idle once it stops.
    #[test]
    fn the_rate_meter_lights_up_then_settles() {
        let mut meter = RateMeter::default();
        assert!(
            !meter.sample(1, 0, 0),
            "the first sample is only a baseline"
        );
        assert!(meter.sample(1, 4096, 0), "a write must light the up arrow");
        let mut settled = None;
        for frame in 0..64 {
            if !meter.sample(1, 4096, 0) {
                settled = Some(frame);
                break;
            }
        }
        assert!(settled.is_some(), "the rate never decayed to idle");
        assert!(meter.up_per_sec <= TRAFFIC_IDLE);
        assert_eq!(meter.down_per_sec, 0.0);
    }

    /// A background tab that receives output is underlined, and looking at it
    /// clears the mark.
    #[test]
    fn a_background_tab_is_marked_until_it_is_looked_at() {
        use std::sync::atomic::Ordering;
        let ctx = egui::Context::default();
        let mut app = App::from_settings(&ctx, Settings::default(), None, None);
        let size = Vec2::new(1280.0, 800.0);
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
        app.open_connecting_tab(SessionKind::Serial(SerialProfile {
            port: "COM199".into(),
            ..Default::default()
        }));
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active, 1, "the new tab is the active one");
        assert!(!tab_updated(&app.tabs[1], true), "the active tab is clean");

        // The first tab reads something while it is off screen.
        app.tabs[0].panes[0]
            .session
            .traffic
            .down
            .fetch_add(64, Ordering::Relaxed);
        assert!(tab_updated(&app.tabs[0], false), "new output must mark it");

        app.active = 0;
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
        assert!(
            !tab_updated(&app.tabs[0], true),
            "looking at the tab must clear the mark"
        );
        assert_eq!(app.tabs[0].seen_output, tab_output(&app.tabs[0]));
    }

    /// The toolbox types Linux commands, so it must not open on a local shell.
    #[test]
    fn the_toolbox_refuses_a_session_that_is_not_ssh() {
        let ctx = egui::Context::default();
        let mut app = App::from_settings(&ctx, Settings::default(), None, None);
        let size = Vec2::new(1280.0, 800.0);
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
        app.execute(Action::Toolbox, &ctx);
        assert!(app.toolbox.is_none());
        assert_eq!(
            app.error.as_deref(),
            Some("服务器工具箱仅用于已连接的 SSH 会话")
        );
    }

    #[test]
    fn bitrates_use_decimal_units() {
        assert_eq!(format_bitrate(0.0), "0 bps");
        assert_eq!(format_bitrate(1.0), "8 bps");
        assert_eq!(format_bitrate(125.0), "1.0 Kbps");
        assert_eq!(format_bitrate(1_000_000.0), "8.0 Mbps");
    }

    /// Alt+C drops the session but keeps the pane and its screen, so Alt+R has
    /// something to bring back.
    #[test]
    fn the_disconnect_shortcut_keeps_the_pane_and_reconnects() {
        let ctx = egui::Context::default();
        let mut app = App::from_settings(&ctx, Settings::default(), None, None);
        let size = Vec2::new(1280.0, 800.0);
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
        let before = app.tabs[0].panes[0].session.terminal.clone();
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        frame(&mut app, &ctx, vec![key(Key::C, alt)], alt, size);
        assert_eq!(app.tabs.len(), 1, "断开 must not close the pane");
        let session = &app.tabs[0].panes[0].session;
        assert_eq!(session.link(), SessionStatus::Detached);
        assert!(
            Arc::ptr_eq(&before, &session.terminal),
            "断开 must keep the screen"
        );
        assert!(
            session
                .terminal
                .lock()
                .unwrap()
                .parser
                .screen()
                .contents()
                .contains("已断开")
        );

        frame(&mut app, &ctx, vec![key(Key::R, alt)], alt, size);
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.tabs[0].panes[0].session.link(), SessionStatus::Live);
    }

    /// Alt+R arrives as a key event *and* as text. The key is the shortcut; the
    /// text must not be typed into the session — on an ended session that was
    /// reported as a spurious write error right after reconnecting.
    #[test]
    fn a_shortcut_does_not_leak_its_text_into_the_session() {
        let ctx = egui::Context::default();
        let mut app = App::from_settings(&ctx, Settings::default(), None, None);
        let size = Vec2::new(1280.0, 800.0);
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
        app.execute(Action::Disconnect, &ctx);
        assert_eq!(app.tabs[0].panes[0].session.link(), SessionStatus::Detached);

        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        frame(
            &mut app,
            &ctx,
            vec![key(Key::R, alt), Event::Text("r".into())],
            alt,
            size,
        );
        assert!(
            app.error.is_none(),
            "the shortcut's text reached the ended session: {:?}",
            app.error
        );
        assert_eq!(
            app.tabs[0].panes[0].session.link(),
            SessionStatus::Live,
            "Alt+R must still reconnect"
        );
    }

    #[test]
    fn a_toast_stays_for_its_lifetime_then_goes() {
        let ctx = egui::Context::default();
        let mut app = App::from_settings(&ctx, Settings::default(), None, None);
        let size = Vec2::new(1280.0, 800.0);
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
        app.notify("已复制 3 个字符");
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
        assert!(app.toast.is_some(), "an unexpired toast must stay");
        app.toast = Some((
            "old".into(),
            std::time::Instant::now() - TOAST_LIFETIME - std::time::Duration::from_secs(1),
        ));
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
        assert!(app.toast.is_none(), "an expired toast must be dropped");
    }

    /// A connection gets its tab before it is attempted, and a failure lands in
    /// that tab's own console instead of a status line shared by every session.
    /// COM199 is used because it cannot exist, so no device is touched.
    #[test]
    fn a_failed_connection_reports_in_its_own_console() {
        let ctx = egui::Context::default();
        let mut app = App::from_settings(&ctx, Settings::default(), None, None);
        let size = Vec2::new(1280.0, 800.0);
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
        let before = app.tabs.len();
        app.execute(
            Action::New(SessionKind::Serial(SerialProfile {
                port: "COM199".into(),
                ..Default::default()
            })),
            &ctx,
        );
        assert_eq!(
            app.tabs.len(),
            before + 1,
            "the tab must exist before the connect attempt"
        );
        let terminal = app.tabs.last().unwrap().panes[0].session.terminal.clone();
        let text = terminal.lock().unwrap().parser.screen().contents();
        assert!(text.contains("connect to COM199"), "{text}");
        assert!(text.contains("connect failed"), "{text}");
        assert!(
            app.error.is_none(),
            "a connection failure belongs to its own console: {:?}",
            app.error
        );
    }

    /// The console offers its clear actions on right-click, and the menu has to
    /// survive more than the frame that opened it.
    #[test]
    fn right_click_opens_the_console_menu_and_it_stays_open() {
        let ctx = egui::Context::default();
        let mut app = App::from_settings(&ctx, Settings::default(), None, None);
        let size = Vec2::new(1280.0, 800.0);
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
        let pos = egui::Pos2::new(700.0, 400.0);
        frame(
            &mut app,
            &ctx,
            vec![Event::PointerMoved(pos)],
            Modifiers::NONE,
            size,
        );
        for pressed in [true, false] {
            frame(
                &mut app,
                &ctx,
                vec![Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Secondary,
                    pressed,
                    modifiers: Modifiers::NONE,
                }],
                Modifiers::NONE,
                size,
            );
        }
        assert!(egui::Popup::is_any_open(&ctx), "the menu did not open");
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
        assert!(
            egui::Popup::is_any_open(&ctx),
            "the menu closed on the next frame"
        );
    }

    /// The serial picker and the serial tab of the connection form only ever
    /// list devices; opening a port happens on connect. A port that is not
    /// attached is used here precisely so the test cannot touch real hardware.
    #[test]
    fn serial_picker_and_form_open_without_touching_a_device() {
        let ctx = egui::Context::default();
        let mut app = App::from_settings(&ctx, Settings::default(), None, None);
        let size = Vec2::new(1280.0, 800.0);
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);

        app.execute(Action::SerialPicker, &ctx);
        assert!(app.serial_picker.is_some());
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);

        app.execute(Action::Remote, &ctx);
        app.profile_kind = ProfileKind::Serial;
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
        assert!(app.remote_open);

        app.settings.serial_profiles.push(SerialProfile {
            port: "COM199".into(),
            ..Default::default()
        });
        app.execute(Action::EditSerial(0), &ctx);
        assert_eq!(app.profile_kind, ProfileKind::Serial);
        assert_eq!(app.serial.port, "COM199");
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);

        app.execute(Action::RemoveSerial(0), &ctx);
        assert!(app.settings.serial_profiles.is_empty());
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
