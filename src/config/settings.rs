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
    /// Most recently opened saved or imported connections, newest first.
    pub recent_connections: Vec<String>,
    /// Tab tag colours by group name, `#rrggbb`. A connection's own colour wins
    /// over its group's; an empty entry means no colour.
    pub group_colors: std::collections::BTreeMap<String, String>,
    pub copy_on_select: bool,
    /// Disconnect serial sessions after this many minutes in a hidden tab or unfocused window.
    pub serial_background_timeout_enabled: bool,
    pub serial_background_timeout_minutes: u32,
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
    /// This window's view of the file as of its last load or save. Another
    /// window may have saved since; [`merge`] uses this to tell what *this*
    /// window changed, so only those changes are laid over the file.
    #[serde(skip)]
    #[doc(hidden)]
    pub baseline: Option<serde_json::Value>,
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
            recent_connections: Vec::new(),
            group_colors: std::collections::BTreeMap::new(),
            copy_on_select: false,
            serial_background_timeout_enabled: false,
            serial_background_timeout_minutes: 5,
            hide_dotfiles: true,
            restore_tabs: true,
            confirm_on_exit: true,
            auto_hide_sidebar: false,
            workspace: Vec::new(),
            ssh_config_profiles: Vec::new(),
            baseline: None,
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
        // Every persisted field except `workspace`, which is read tab by tab below.
        macro_rules! read_fields {
            ($($field:ident),* $(,)?) => {$(
                if let Some(v) = read_field(&value, stringify!($field)) {
                    settings.$field = v;
                }
            )*};
        }
        read_fields!(
            font_size,
            scrollback,
            light_theme,
            sidebar,
            default_shell,
            search_engine,
            profiles,
            serial_profiles,
            groups,
            recent_connections,
            group_colors,
            copy_on_select,
            serial_background_timeout_enabled,
            serial_background_timeout_minutes,
            hide_dotfiles,
            restore_tabs,
            confirm_on_exit,
            auto_hide_sidebar,
        );
        settings.serial_background_timeout_minutes =
            settings.serial_background_timeout_minutes.clamp(1, 1440);
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
        settings.baseline = serde_json::to_value(&settings).ok();
        Ok(settings)
    }

    /// Writes this window's changes over whatever is on disk now.
    ///
    /// Every window is its own process with its own copy of the settings. A
    /// plain overwrite let one window's stale copy erase a connection another
    /// window had just saved, so the file is re-read and only what changed
    /// here since the last load or save is applied on top of it.
    pub fn save(&mut self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mine = serde_json::to_value(&*self)?;
        let merged = match (
            &self.baseline,
            std::fs::read(&path)
                .ok()
                .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok()),
        ) {
            (Some(base), Some(disk)) => merge(base, &mine, &disk),
            _ => mine.clone(),
        };
        // Stage a complete file before replacing the previous configuration.
        // The pid keeps two windows saving at once from sharing a staging file.
        let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&temp, serde_json::to_vec_pretty(&merged)?)?;
        std::fs::rename(&temp, &path).context("无法保存设置")?;
        self.baseline = Some(mine);
        Ok(())
    }
}

/// Three-way merge of one window's settings onto the file. A field this window
/// did not touch keeps the file's value; a list it did touch gets this
/// window's additions and removals applied to the file's list, so connections
/// saved by two windows both survive. Ordered or per-window fields
/// (`recent_connections`, `workspace`) and plain values take this window's
/// value when it changed them.
pub(super) fn merge(
    base: &serde_json::Value,
    mine: &serde_json::Value,
    disk: &serde_json::Value,
) -> serde_json::Value {
    use serde_json::Value;
    let (Value::Object(mine_map), Value::Object(disk_map)) = (mine, disk) else {
        return mine.clone();
    };
    let mut out = disk_map.clone();
    for (key, mine_value) in mine_map {
        let base_value = base.get(key);
        if base_value == Some(mine_value) {
            // Untouched here: whatever the file has wins, including a value
            // another window wrote.
            out.entry(key.clone()).or_insert_with(|| mine_value.clone());
            continue;
        }
        let merged = match (key.as_str(), base_value, mine_value, disk_map.get(key)) {
            ("recent_connections" | "workspace", ..) => mine_value.clone(),
            (_, Some(Value::Array(base)), Value::Array(mine), Some(Value::Array(disk))) => {
                let mut list: Vec<Value> = disk
                    .iter()
                    .filter(|item| mine.contains(item) || !base.contains(item))
                    .cloned()
                    .collect();
                for item in mine {
                    if !base.contains(item) && !list.contains(item) {
                        list.push(item.clone());
                    }
                }
                Value::Array(list)
            }
            (_, Some(Value::Object(base)), Value::Object(mine), Some(Value::Object(disk))) => {
                let mut map = disk.clone();
                for (k, v) in mine {
                    if base.get(k) != Some(v) {
                        map.insert(k.clone(), v.clone());
                    }
                }
                for k in base.keys() {
                    if !mine.contains_key(k) {
                        map.remove(k);
                    }
                }
                Value::Object(map)
            }
            _ => mine_value.clone(),
        };
        out.insert(key.clone(), merged);
    }
    Value::Object(out)
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
