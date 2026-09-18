//! The settings file: its defaults, on-disk location and tolerant parsing.

use super::*;

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
    pub(super) fn parse(bytes: &[u8]) -> Result<Self> {
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
pub(super) fn read_field<T: serde::de::DeserializeOwned>(
    value: &serde_json::Value,
    key: &str,
) -> Option<T> {
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
