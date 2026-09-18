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
pub use ssh::{load_ssh_config, parse_ssh_config, ssh_config_path};
