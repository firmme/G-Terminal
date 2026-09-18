//! Importing hosts from `~/.ssh/config`.

use super::*;

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
pub(super) fn resolve_jump_aliases(profiles: &mut [RemoteProfile]) {
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
pub(super) struct SshConfigBlock {
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
    pub(super) fn set(&mut self, key: &str, value: &str) {
        match key.to_ascii_lowercase().as_str() {
            "hostname" if self.hostname.is_none() => self.hostname = Some(value.to_string()),
            "user" if self.user.is_none() => self.user = Some(value.to_string()),
            "port" if self.port.is_none() => self.port = value.parse().ok(),
            "identityfile" if self.identity.is_none() => self.identity = Some(expand_home(value)),
            "proxyjump" if self.jump.is_none() => self.jump = parse_proxy_jump(value),
            _ => {}
        }
    }

    pub(super) fn into_profiles(self) -> Option<Vec<RemoteProfile>> {
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
pub(super) fn parse_proxy_jump(value: &str) -> Option<RemoteProfile> {
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
pub(super) fn ssh_directive(raw: &str) -> Option<(String, String)> {
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
pub(super) fn strip_comment(line: &str) -> &str {
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

pub(super) fn unquote(value: &str) -> String {
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
pub(super) fn expand_home(value: &str) -> String {
    let Some(home) = directories::UserDirs::new().map(|dirs| PathBuf::from(dirs.home_dir())) else {
        return value.to_string();
    };
    match value.strip_prefix("~/") {
        Some(rest) => home.join(rest).to_string_lossy().into_owned(),
        None if value == "~" => home.to_string_lossy().into_owned(),
        None => value.to_string(),
    }
}
