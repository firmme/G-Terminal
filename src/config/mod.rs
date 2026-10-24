use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Colours offered as tag presets, as `#rrggbb`. Nothing is picked by default,
/// so a connection or group stays transparent until one is chosen.
pub const TAG_COLORS: [&str; 9] = [
    "#e06c75", "#e5c07b", "#98c379", "#56b6c2", "#61afef", "#c678dd", "#ff9e64", "#f783ac",
    "#8bd5ca",
];

mod profile;
mod search;
mod settings;
mod ssh;
#[cfg(test)]
mod tests;
pub use profile::{BAUD_RATES, DEFAULT_BAUD, Forward, RemoteProfile, SerialProfile};
pub use search::{DEFAULT_SEARCH_ENGINE, SEARCH_ENGINES, search_engine_label, search_url};
pub use settings::Settings;
/// Expands a leading `~` (`~`, `~/x`, `~\x`) to the home directory, as OpenSSH
/// does for key paths. Surrounding whitespace is dropped; anything else is
/// returned as written.
pub fn expand_home(path: &str) -> PathBuf {
    let path = path.trim();
    let home = || directories::UserDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
    if path == "~" {
        return home().unwrap_or_else(|| PathBuf::from(path));
    }
    match (
        path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")),
        home(),
    ) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

pub use ssh::{load_ssh_config, parse_ssh_config, ssh_config_path};
