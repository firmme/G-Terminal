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
pub const HOME_TOKEN: &str = "$USERPROFILE$";

/// A portable home-relative path may never escape the receiving user's home.
fn home_relative(path: &str) -> Option<String> {
    let path = path.trim();
    let rest = if path == "~" {
        ""
    } else if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        rest
    } else if path
        .get(..HOME_TOKEN.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(HOME_TOKEN))
    {
        let rest = &path[HOME_TOKEN.len()..];
        if rest.is_empty() {
            rest
        } else {
            rest.strip_prefix(['/', '\\'])?
        }
    } else {
        return None;
    };
    let rest = rest.replace('\\', "/");
    if rest.starts_with('/')
        || rest.contains(':')
        || rest.chars().any(char::is_control)
        || rest.split('/').any(|part| part == "..")
    {
        return None;
    }
    Some(
        rest.split('/')
            .filter(|p| !p.is_empty() && *p != ".")
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// Convert only paths inside this user's home to a portable, canonical token.
/// No filesystem access is needed: missing keys can still be synced and repaired.
pub fn portable_identity(path: &str) -> Option<String> {
    let relative = home_relative(path).or_else(|| {
        let dirs = directories::UserDirs::new()?;
        let home = dirs.home_dir().to_string_lossy().replace('\\', "/");
        let path = path.trim().replace('\\', "/");
        let home = home.trim_end_matches('/');
        let prefix = path.get(..home.len())?;
        let matches = if cfg!(windows) {
            prefix.eq_ignore_ascii_case(home)
        } else {
            prefix == home
        };
        if !matches {
            return None;
        }
        let rest = path.get(home.len()..)?.strip_prefix('/')?;
        home_relative(&format!("{HOME_TOKEN}/{rest}"))
    })?;
    Some(if relative.is_empty() {
        HOME_TOKEN.into()
    } else {
        format!("{HOME_TOKEN}/{relative}")
    })
}

/// Expand `~` or `$USERPROFILE$` using this machine's home on every platform.
pub fn expand_home(path: &str) -> PathBuf {
    let path = path.trim();
    match (home_relative(path), directories::UserDirs::new()) {
        (Some(rest), Some(home)) => home.home_dir().join(rest),
        _ => PathBuf::from(path),
    }
}

pub use ssh::{load_ssh_config, parse_ssh_config, ssh_config_path};
