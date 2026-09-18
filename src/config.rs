use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub font_size: f32,
    pub scrollback: usize,
    pub light_theme: bool,
    pub sidebar: bool,
    pub default_shell: String,
    /// Which web search engine the terminal's selection lookup opens.
    pub search_engine: String,
    pub profiles: Vec<RemoteProfile>,
    pub serial_profiles: Vec<SerialProfile>,
    pub groups: Vec<String>,
    /// Tab tag colours by group name, `#rrggbb`. A connection's own colour wins
    /// over its group's; an empty entry means no colour.
    pub group_colors: std::collections::BTreeMap<String, String>,
    pub copy_on_select: bool,
    pub hide_dotfiles: bool,
    /// Restore the tabs that were open when the program last closed, as
    /// disconnected panes: no shell is started and no host is contacted.
    pub restore_tabs: bool,
    /// Ask before closing the window while a session is still connected.
    pub confirm_on_exit: bool,
    /// Keep the navigation bar out of the layout and reveal it on hover.
    pub auto_hide_sidebar: bool,
    pub workspace: Vec<crate::layout::SavedTab>,
    /// Hosts read from `~/.ssh/config` at startup. Never persisted: the file is
    /// re-read on every launch, so an edit there is never shadowed by a stale
    /// copy in `settings.json`.
    #[serde(skip)]
    pub ssh_config_profiles: Vec<RemoteProfile>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_size: 15.0,
            scrollback: 10_000,
            light_theme: false,
            sidebar: true,
            default_shell: if cfg!(windows) { "powershell" } else { "shell" }.into(),
            search_engine: DEFAULT_SEARCH_ENGINE.into(),
            profiles: Vec::new(),
            serial_profiles: Vec::new(),
            groups: Vec::new(),
            group_colors: std::collections::BTreeMap::new(),
            copy_on_select: false,
            hide_dotfiles: true,
            restore_tabs: true,
            confirm_on_exit: true,
            auto_hide_sidebar: false,
            workspace: Vec::new(),
            ssh_config_profiles: Vec::new(),
        }
    }
}

/// Colours offered as tag presets, as `#rrggbb`. Nothing is picked by default,
/// so a connection or group stays transparent until one is chosen.
pub const TAG_COLORS: [&str; 9] = [
    "#e06c75", "#e5c07b", "#98c379", "#56b6c2", "#61afef", "#c678dd", "#ff9e64", "#f783ac",
    "#8bd5ca",
];

/// The engine used when settings carry none.
pub const DEFAULT_SEARCH_ENGINE: &str = "google";

/// Web search engines offered for the terminal's selection lookup. The first
/// field is the key stored in settings; the third is the URL template, with
/// `{}` standing for the percent-encoded query.
pub const SEARCH_ENGINES: [(&str, &str, &str); 4] = [
    ("google", "Google", "https://www.google.com/search?q={}"),
    ("bing", "Bing", "https://www.bing.com/search?q={}"),
    ("duckduckgo", "DuckDuckGo", "https://duckduckgo.com/?q={}"),
    ("baidu", "百度", "https://www.baidu.com/s?wd={}"),
];

/// The display name for a stored engine key.
pub fn search_engine_label(engine: &str) -> &str {
    SEARCH_ENGINES
        .iter()
        .find(|(key, _, _)| *key == engine)
        .map(|(_, label, _)| *label)
        .unwrap_or(engine)
}

/// The search URL for `query` on `engine`. An unknown key falls back to the
/// default engine, so a hand-edited config cannot produce a broken lookup.
pub fn search_url(engine: &str, query: &str) -> String {
    let template = SEARCH_ENGINES
        .iter()
        .find(|(key, _, _)| *key == engine)
        .or_else(|| {
            SEARCH_ENGINES
                .iter()
                .find(|(key, _, _)| *key == DEFAULT_SEARCH_ENGINE)
        })
        .map(|(_, _, template)| *template)
        .unwrap_or("https://www.google.com/search?q={}");
    template.replace("{}", &percent_encode(query))
}

/// Percent-encodes a query for a URL: RFC 3986 unreserved characters stay, and
/// everything else — including the UTF-8 bytes of CJK — is escaped.
fn percent_encode(query: &str) -> String {
    let mut out = String::with_capacity(query.len());
    for byte in query.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteProfile {
    pub name: String,
    pub host: String,
    pub user: String,
    pub port: u16,
    pub identity: String,
    pub group: String,
    /// Tab tag colour, `#rrggbb`; empty means none. Beats the group's colour.
    pub color: String,
    pub jump: Option<Box<RemoteProfile>>,
    pub forwards: Vec<Forward>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Forward {
    pub bind_port: u16,
    pub target_host: String,
    pub target_port: u16,
}

impl Default for RemoteProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            host: String::new(),
            user: String::new(),
            port: 22,
            identity: String::new(),
            group: String::new(),
            color: String::new(),
            jump: None,
            forwards: Vec::new(),
        }
    }
}

impl RemoteProfile {
    pub fn validate(&self) -> Result<()> {
        if self.host.is_empty()
            || self.host.starts_with('-')
            || self
                .host
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '@' | '/' | '\\'))
        {
            bail!("请输入有效的主机名或 IP 地址（不能包含空格或命令参数）");
        }
        if self.user.starts_with('-')
            || self
                .user
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '@' | '/' | '\\' | ':'))
        {
            bail!("用户名不能包含空格、路径或命令参数");
        }
        if self.port == 0 {
            bail!("端口范围是 1–65535");
        }
        if let Some(jump) = &self.jump {
            if jump.jump.is_some() {
                bail!("ProxyJump 当前只支持一级跳板机");
            }
            jump.validate()?;
        }
        let mut ports = std::collections::HashSet::new();
        for forward in &self.forwards {
            if forward.bind_port == 0
                || forward.target_port == 0
                || forward.target_host.is_empty()
                || forward
                    .target_host
                    .chars()
                    .any(|c| c.is_control() || c.is_whitespace())
            {
                bail!("请输入有效的转发地址和端口");
            }
            if !ports.insert(forward.bind_port) {
                bail!("同一个连接不能重复监听本地端口 {}", forward.bind_port);
            }
        }
        Ok(())
    }

    pub fn destination(&self) -> String {
        if self.user.is_empty() {
            self.host.clone()
        } else {
            format!("{}@{}", self.user, self.host)
        }
    }

    pub fn label(&self) -> String {
        if self.name.trim().is_empty() {
            self.host.clone()
        } else {
            self.name.clone()
        }
    }
}

/// `~/.ssh/config`, when the platform can name a home directory.
pub fn ssh_config_path() -> Option<PathBuf> {
    directories::UserDirs::new()
        .map(|dirs| PathBuf::from(dirs.home_dir()).join(".ssh").join("config"))
}

/// Reads and parses `~/.ssh/config`. A missing or unreadable file is not an
/// error — not having one is the normal state.
pub fn load_ssh_config() -> Vec<RemoteProfile> {
    match ssh_config_path().and_then(|path| std::fs::read_to_string(path).ok()) {
        Some(text) => parse_ssh_config(&text),
        None => Vec::new(),
    }
}

/// Parses an OpenSSH client config, keeping only `Host` blocks that carry a
/// `HostName`. Every non-wildcard alias on the `Host` line becomes its own
/// connection pointing at that hostname, which is what the user typed to reach
/// it. Global options before the first `Host` line, `Include` and `Match`
/// blocks are ignored: this is a connection list, not an OpenSSH client.
pub fn parse_ssh_config(text: &str) -> Vec<RemoteProfile> {
    let mut profiles = Vec::new();
    let mut block = SshConfigBlock::default();
    for raw in text.lines() {
        let Some((key, value)) = ssh_directive(raw) else {
            continue;
        };
        if key.eq_ignore_ascii_case("Host") {
            if let Some(parsed) = block.into_profiles() {
                profiles.extend(parsed);
            }
            block = SshConfigBlock {
                started: true,
                aliases: value.split_whitespace().map(str::to_string).collect(),
                ..Default::default()
            };
            continue;
        }
        if !block.started {
            continue;
        }
        block.set(&key, &value);
    }
    if let Some(parsed) = block.into_profiles() {
        profiles.extend(parsed);
    }
    resolve_jump_aliases(&mut profiles);
    profiles
}

/// A `ProxyJump` normally names another alias, and OpenSSH resolves it against
/// the same file. This client does not read configs at connect time, so the
/// alias is replaced with the hostname it stands for, losing only the nested
/// jump (which this client does not support anyway).
fn resolve_jump_aliases(profiles: &mut [RemoteProfile]) {
    let targets: Vec<(String, RemoteProfile)> = profiles
        .iter()
        .map(|profile| (profile.name.clone(), profile.clone()))
        .collect();
    for profile in profiles.iter_mut() {
        let Some(jump) = profile.jump.as_mut() else {
            continue;
        };
        let Some((_, target)) = targets.iter().find(|(name, _)| *name == jump.host) else {
            continue;
        };
        jump.host = target.host.clone();
        if jump.user.is_empty() {
            jump.user = target.user.clone();
        }
        if jump.port == 22 {
            jump.port = target.port;
        }
        if jump.identity.is_empty() {
            jump.identity = target.identity.clone();
        }
    }
}

/// One `Host` block, collected before it is turned into profiles.
#[derive(Default)]
struct SshConfigBlock {
    started: bool,
    aliases: Vec<String>,
    hostname: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    identity: Option<String>,
    jump: Option<RemoteProfile>,
}

impl SshConfigBlock {
    /// First value wins, matching OpenSSH's own resolution order.
    fn set(&mut self, key: &str, value: &str) {
        match key.to_ascii_lowercase().as_str() {
            "hostname" if self.hostname.is_none() => self.hostname = Some(value.to_string()),
            "user" if self.user.is_none() => self.user = Some(value.to_string()),
            "port" if self.port.is_none() => self.port = value.parse().ok(),
            "identityfile" if self.identity.is_none() => self.identity = Some(expand_home(value)),
            "proxyjump" if self.jump.is_none() => self.jump = parse_proxy_jump(value),
            _ => {}
        }
    }

    fn into_profiles(self) -> Option<Vec<RemoteProfile>> {
        let hostname = self.hostname?;
        if hostname.trim().is_empty() {
            return None;
        }
        // A wildcard pattern is a rule, not an address to connect to.
        let mut profiles = Vec::new();
        for alias in self.aliases {
            if alias.contains('*') || alias.contains('?') {
                continue;
            }
            profiles.push(RemoteProfile {
                name: alias,
                host: hostname.clone(),
                user: self.user.clone().unwrap_or_default(),
                port: self.port.unwrap_or(22),
                identity: self.identity.clone().unwrap_or_default(),
                group: String::new(),
                color: String::new(),
                jump: self.jump.clone().map(Box::new),
                forwards: Vec::new(),
            });
        }
        (!profiles.is_empty()).then_some(profiles)
    }
}

/// The first hop of a `ProxyJump`, which is all this client supports.
fn parse_proxy_jump(value: &str) -> Option<RemoteProfile> {
    let first = value.split(',').next()?.trim();
    if first.is_empty() {
        return None;
    }
    let (user, rest) = match first.split_once('@') {
        Some((user, rest)) => (user.to_string(), rest),
        None => (String::new(), first),
    };
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) => match port.parse::<u16>() {
            Ok(port) => (host.to_string(), port),
            Err(_) => (rest.to_string(), 22),
        },
        None => (rest.to_string(), 22),
    };
    if host.trim().is_empty() {
        return None;
    }
    Some(RemoteProfile {
        host,
        user,
        port,
        ..Default::default()
    })
}

/// Splits one config line into a directive and its value, honouring comments
/// and the `Key value` / `Key=value` forms.
fn ssh_directive(raw: &str) -> Option<(String, String)> {
    let line = strip_comment(raw);
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let (key, value) = match line.split_once('=') {
        Some((key, value)) if !key.trim().contains(char::is_whitespace) => (key.trim(), value),
        _ => match line.split_once(char::is_whitespace) {
            Some((key, value)) => (key, value),
            None => (line, ""),
        },
    };
    Some((key.to_string(), unquote(value.trim())))
}

/// Drops an unquoted `#` comment: `#` inside quotes is part of the value.
fn strip_comment(line: &str) -> &str {
    let mut quote = None;
    for (index, ch) in line.char_indices() {
        match ch {
            '"' | '\'' if quote == Some(ch) => quote = None,
            '"' | '\'' if quote.is_none() => quote = Some(ch),
            '#' if quote.is_none() => return &line[..index],
            _ => {}
        }
    }
    line
}

fn unquote(value: &str) -> String {
    if value.len() >= 2
        && let Some(inner) = value
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
    {
        return inner.to_string();
    }
    if value.len() >= 2
        && let Some(inner) = value
            .strip_prefix('\'')
            .and_then(|rest| rest.strip_suffix('\''))
    {
        return inner.to_string();
    }
    value.to_string()
}

/// Expands a leading `~` in a config path, which OpenSSH does for
/// `IdentityFile`.
fn expand_home(value: &str) -> String {
    let Some(home) = directories::UserDirs::new().map(|dirs| PathBuf::from(dirs.home_dir())) else {
        return value.to_string();
    };
    match value.strip_prefix("~/") {
        Some(rest) => home.join(rest).to_string_lossy().into_owned(),
        None if value == "~" => home.to_string_lossy().into_owned(),
        None => value.to_string(),
    }
}

/// Line speeds offered as presets. 115200 is the default, and is what a
/// console cable almost always expects.
pub const BAUD_RATES: [u32; 8] = [9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600];

pub const DEFAULT_BAUD: u32 = 115_200;

/// A serial port the terminal can open directly, with no process on the other
/// end of it. `port` is the device name (`COM3`, `/dev/ttyUSB0`) or `auto`,
/// which picks a connected device at connect time.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SerialProfile {
    pub name: String,
    pub port: String,
    pub baud: u32,
    pub group: String,
    /// Tab tag colour, `#rrggbb`; empty means none. Beats the group's colour.
    pub color: String,
}

impl Default for SerialProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            port: "auto".into(),
            baud: DEFAULT_BAUD,
            group: String::new(),
            color: String::new(),
        }
    }
}

impl SerialProfile {
    pub fn validate(&self) -> Result<()> {
        let port = self.port.trim();
        if port.is_empty() {
            bail!(
                "请输入串口设备名，例如 {} 或 auto",
                crate::serial::PORT_EXAMPLE
            );
        }
        if port
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '"')
        {
            bail!("串口设备名不能包含空格或引号");
        }
        if self.baud == 0 || self.baud > 4_000_000 {
            bail!("波特率范围是 1–4000000");
        }
        Ok(())
    }

    /// Whether the port is chosen when connecting rather than pinned here.
    pub fn auto(&self) -> bool {
        self.port.trim().eq_ignore_ascii_case("auto")
    }

    pub fn label(&self) -> String {
        if self.name.trim().is_empty() {
            self.port.trim().to_string()
        } else {
            self.name.clone()
        }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        directories::ProjectDirs::from("dev", "gterminal", "G-Terminal")
            .map(|d| d.config_dir().join("settings.json"))
            .unwrap_or_else(|| PathBuf::from("settings.json"))
    }

    /// Reads a settings file field by field. One value that no longer
    /// deserialises — a saved workspace naming a session kind this build does
    /// not know, for instance — is dropped on its own instead of taking every
    /// saved connection down with it. Losing the connections to a stale tab
    /// layout was bad enough to be worth the verbosity.
    fn parse(bytes: &[u8]) -> Result<Self> {
        let value: serde_json::Value =
            serde_json::from_slice(bytes).context("设置不是有效的 JSON")?;
        let mut settings = Self::default();
        if let Some(v) = read_field(&value, "font_size") {
            settings.font_size = v;
        }
        if let Some(v) = read_field(&value, "scrollback") {
            settings.scrollback = v;
        }
        if let Some(v) = read_field(&value, "light_theme") {
            settings.light_theme = v;
        }
        if let Some(v) = read_field(&value, "sidebar") {
            settings.sidebar = v;
        }
        if let Some(v) = read_field(&value, "default_shell") {
            settings.default_shell = v;
        }
        if let Some(v) = read_field(&value, "search_engine") {
            settings.search_engine = v;
        }
        if let Some(v) = read_field(&value, "profiles") {
            settings.profiles = v;
        }
        if let Some(v) = read_field(&value, "serial_profiles") {
            settings.serial_profiles = v;
        }
        if let Some(v) = read_field(&value, "groups") {
            settings.groups = v;
        }
        if let Some(v) = read_field(&value, "group_colors") {
            settings.group_colors = v;
        }
        if let Some(v) = read_field(&value, "copy_on_select") {
            settings.copy_on_select = v;
        }
        if let Some(v) = read_field(&value, "hide_dotfiles") {
            settings.hide_dotfiles = v;
        }
        if let Some(v) = read_field(&value, "restore_tabs") {
            settings.restore_tabs = v;
        }
        if let Some(v) = read_field(&value, "confirm_on_exit") {
            settings.confirm_on_exit = v;
        }
        if let Some(v) = read_field(&value, "auto_hide_sidebar") {
            settings.auto_hide_sidebar = v;
        }
        if let Some(entries) = value.get("workspace").and_then(|v| v.as_array()) {
            // A tab that cannot be rebuilt is skipped; the others still are.
            settings.workspace = entries
                .iter()
                .filter_map(|entry| serde_json::from_value(entry.clone()).ok())
                .collect();
        }
        Ok(settings)
    }

    pub fn load() -> Result<Self> {
        let path = Self::path();
        let mut settings = if !path.exists() {
            Self::default()
        } else {
            let raw = std::fs::read(&path)?;
            match Self::parse(&raw) {
                Ok(mut settings) => {
                    settings.font_size = settings.font_size.clamp(10.0, 28.0);
                    settings.scrollback = settings.scrollback.clamp(100, 50_000);
                    settings
                }
                Err(e) => {
                    // A file that cannot be read at all must not be replaced by
                    // the defaults this failure returns, so it is put aside for
                    // recovery instead of being overwritten on the next save.
                    let backup = path.with_extension("json.broken");
                    let _ = std::fs::rename(&path, &backup);
                    return Err(e).with_context(|| format!("无法读取设置 {}", path.display()));
                }
            }
        };
        // The SSH config is authoritative and never stored, so it is (re)read
        // here rather than parsed out of the settings file.
        settings.ssh_config_profiles = load_ssh_config();
        Ok(settings)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Stage a complete file before replacing the previous configuration.
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&temp, &path).context("无法保存设置")
    }
}

/// One settings field, on its own. A failure is logged and the caller keeps
/// its default rather than the whole file being rejected.
fn read_field<T: serde::de::DeserializeOwned>(value: &serde_json::Value, key: &str) -> Option<T> {
    let raw = value.get(key)?;
    match serde_json::from_value(raw.clone()) {
        Ok(parsed) => Some(parsed),
        Err(_) => {
            use std::io::Write;
            let _ = writeln!(std::io::stderr(), "[设置] 忽略无法解析的字段 {key}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_config_migrates_and_default_label_is_host() {
        let s: Settings = serde_json::from_str(
            r#"{"profiles":[{"name":"","host":"10.0.0.8","user":"root","port":22,"identity":""}]}"#,
        )
        .unwrap();
        assert_eq!(s.profiles[0].label(), "10.0.0.8");
        assert!(s.profiles[0].group.is_empty());
        assert!(!s.copy_on_select);
        assert!(s.restore_tabs);
        assert!(s.serial_profiles.is_empty());
        // A config that predates the field still gets the default engine.
        assert_eq!(s.search_engine, DEFAULT_SEARCH_ENGINE);
    }

    #[test]
    fn colors_round_trip_and_default_to_none() {
        let mut settings = Settings::default();
        // Nothing is coloured until it is picked.
        assert!(settings.profiles.is_empty());
        settings
            .group_colors
            .insert("prod".into(), TAG_COLORS[0].into());
        settings.profiles.push(RemoteProfile {
            group: "prod".into(),
            color: TAG_COLORS[2].into(),
            ..Default::default()
        });
        let json = serde_json::to_string(&settings).unwrap();
        let back = Settings::parse(json.as_bytes()).unwrap();
        assert_eq!(
            back.group_colors.get("prod").map(String::as_str),
            Some(TAG_COLORS[0])
        );
        assert_eq!(back.profiles[0].color, TAG_COLORS[2]);
        assert_eq!(RemoteProfile::default().color, "");
        assert_eq!(SerialProfile::default().color, "");
    }

    #[test]
    fn search_urls_follow_the_engine_and_escape_the_query() {
        assert_eq!(
            search_url("google", "hello world"),
            "https://www.google.com/search?q=hello%20world"
        );
        assert_eq!(
            search_url("bing", "a&b"),
            "https://www.bing.com/search?q=a%26b"
        );
        assert_eq!(
            search_url("baidu", "中国"),
            "https://www.baidu.com/s?wd=%E4%B8%AD%E5%9B%BD"
        );
        // An unknown key still produces a working default lookup.
        assert_eq!(search_url("nope", "x"), "https://www.google.com/search?q=x");
        assert_eq!(search_engine_label("baidu"), "百度");
        assert_eq!(search_engine_label("nope"), "nope");
    }

    /// A tab layout this build cannot rebuild used to fail the whole parse,
    /// which silently replaced the saved connections with the defaults.
    #[test]
    fn an_unreadable_workspace_keeps_the_saved_connections() {
        let settings = Settings::parse(
            br#"{
                "profiles":[{"name":"prod","host":"10.0.0.8","user":"root","port":22}],
                "serial_profiles":[{"name":"router","port":"COM3","baud":115200,"group":""}],
                "workspace":[{"sessions":[{"Unknown":{"x":1}}],"layout":{"Leaf":0},"focused":0}]
            }"#,
        )
        .unwrap();
        assert_eq!(settings.profiles.len(), 1);
        assert_eq!(settings.profiles[0].host, "10.0.0.8");
        assert_eq!(settings.serial_profiles.len(), 1);
        assert_eq!(settings.serial_profiles[0].port, "COM3");
        assert!(settings.workspace.is_empty());
    }

    #[test]
    fn only_a_file_that_is_not_json_at_all_is_rejected() {
        assert!(Settings::parse(b"{ not json").is_err());
        // A field with the wrong type costs that field, not the file.
        let settings =
            Settings::parse(br#"{"font_size":"huge","profiles":[{"host":"10.0.0.8"}]}"#).unwrap();
        assert_eq!(settings.font_size, Settings::default().font_size);
        assert_eq!(settings.profiles.len(), 1);
    }

    #[test]
    fn serial_profiles_default_to_auto_115200_and_validate() {
        let profile = SerialProfile::default();
        assert!(profile.auto());
        assert_eq!(profile.baud, DEFAULT_BAUD);
        assert_eq!(profile.label(), "auto");
        assert!(profile.validate().is_ok());

        let pinned = SerialProfile {
            name: "路由器".into(),
            port: " COM3 ".into(),
            ..Default::default()
        };
        assert!(!pinned.auto());
        assert_eq!(pinned.label(), "路由器");
        assert!(pinned.validate().is_ok());

        assert!(
            SerialProfile {
                port: "  ".into(),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            SerialProfile {
                port: "COM 3".into(),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            SerialProfile {
                baud: 0,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn ssh_config_imports_only_host_blocks_with_a_hostname() {
        let config = "\
# ~/.ssh/config
Host *
    ServerAliveInterval 30
Host web web-alias
    HostName web.example.com
    User deploy
    Port 2222
    IdentityFile ~/.ssh/id_web
    ProxyJump bastion
Host wildcard-*
    HostName nope
Host no-hostname
    User nobody
";
        let profiles = parse_ssh_config(config);
        // Only the two aliases of the block that names a host, never `*`.
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].name, "web");
        assert_eq!(profiles[0].host, "web.example.com");
        assert_eq!(profiles[0].user, "deploy");
        assert_eq!(profiles[0].port, 2222);
        assert_eq!(profiles[0].identity, expand_home("~/.ssh/id_web"));
        assert_eq!(profiles[1].name, "web-alias");
        assert_eq!(profiles[1].host, "web.example.com");
        // A global option before the first Host must not leak into the import.
        assert!(profiles.iter().all(|p| p.port != 30));
        let jump = profiles[0].jump.as_ref().unwrap();
        assert_eq!(jump.host, "bastion");
        assert_eq!(jump.port, 22);
    }

    #[test]
    fn ssh_config_accepts_equals_quotes_and_comments() {
        let config = r#"
Host=quoted
  HostName="a#b.example.com" # trailing comment
  Port=2200
"#;
        let profiles = parse_ssh_config(config);
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "quoted");
        assert_eq!(profiles[0].host, "a#b.example.com");
        assert_eq!(profiles[0].port, 2200);
    }

    #[test]
    fn proxy_jump_splits_user_host_port_in_any_combination() {
        let jump = parse_proxy_jump("root@jump.example.com:2022").unwrap();
        assert_eq!(jump.user, "root");
        assert_eq!(jump.host, "jump.example.com");
        assert_eq!(jump.port, 2022);
        let plain = parse_proxy_jump("bastion").unwrap();
        assert_eq!(plain.user, "");
        assert_eq!(plain.host, "bastion");
        assert_eq!(plain.port, 22);
        // Multiple hops: only the first is usable here.
        assert_eq!(parse_proxy_jump("a,b,c").unwrap().host, "a");
        assert!(parse_proxy_jump("  ").is_none());
    }

    #[test]
    fn proxy_jump_aliases_resolve_to_the_imported_host() {
        let config = "\
Host bastion
    HostName jumphost.example.com
    User admin
    Port 2022
Host app
    HostName 10.0.0.5
    ProxyJump bastion
";
        let profiles = parse_ssh_config(config);
        let app = profiles.iter().find(|p| p.name == "app").unwrap();
        let jump = app.jump.as_ref().unwrap();
        assert_eq!(jump.host, "jumphost.example.com");
        assert_eq!(jump.user, "admin");
        assert_eq!(jump.port, 2022);
    }

    #[test]
    fn serial_profiles_round_trip_through_settings_json() {
        let settings = Settings {
            serial_profiles: vec![SerialProfile {
                port: "COM9".into(),
                baud: 57600,
                group: "设备".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.serial_profiles.len(), 1);
        assert_eq!(back.serial_profiles[0].port, "COM9");
        assert_eq!(back.serial_profiles[0].baud, 57600);
        assert_eq!(back.serial_profiles[0].group, "设备");
    }
}
