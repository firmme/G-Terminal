//! Quick SSH targets. Credentials never become part of a saved profile.
use super::*;

#[derive(Clone)]
pub(super) struct Target {
    pub profile: RemoteProfile,
    pub password: Option<zeroize::Zeroizing<String>>,
    pub prompt: bool,
}

pub(super) fn parse(input: &str) -> Option<Target> {
    let input = input.trim();
    if input.is_empty() || input.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let (user, password, address, explicit) =
        if let Some((credentials, address)) = input.rsplit_once('@') {
            let (user, password) = credentials
                .split_once(':')
                .map_or((credentials, None), |(user, password)| {
                    (user, Some(password))
                });
            if user
                .chars()
                .any(|c| !(c.is_ascii_alphanumeric() || "_-.".contains(c)))
            {
                return None;
            }
            (user, password, address, true)
        } else {
            ("", None, input, false)
        };
    let (host, port) = if let Some(bracketed) = address.strip_prefix('[') {
        let (host, tail) = bracketed.split_once(']')?;
        host.parse::<std::net::Ipv6Addr>().ok()?;
        let port = if tail.is_empty() {
            22
        } else {
            tail.strip_prefix(':')?.parse::<u16>().ok()?
        };
        (host, port)
    } else if address.parse::<std::net::IpAddr>().is_ok() {
        (address, 22)
    } else if let Some((host, port)) = address.split_once(':') {
        (host, port.parse::<u16>().ok()?)
    } else {
        (address, 22)
    };
    if port == 0 || host.is_empty() || host.len() > 253 {
        return None;
    }
    let ip = host.parse::<std::net::IpAddr>().is_ok();
    if !ip {
        // Malformed IPs and ordinary search words must remain searches.
        if host.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return None;
        }
        if !explicit && !host.contains('.') && host != "localhost" {
            return None;
        }
        if !host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        }) {
            return None;
        }
    }
    Some(Target {
        profile: RemoteProfile {
            user: user.into(),
            host: host.into(),
            port,
            ..Default::default()
        },
        password: password
            .filter(|password| !password.is_empty())
            .map(|password| zeroize::Zeroizing::new(password.to_owned())),
        prompt: user.is_empty() && password.is_none(),
    })
}

pub(super) fn from_recent_key(key: &str) -> Option<SessionKind> {
    if let Some(key) = key.strip_prefix("ssh:") {
        let (destination, port) = key.rsplit_once(':')?;
        let (user, host) = destination.split_once('@')?;
        Some(SessionKind::Ssh(RemoteProfile {
            user: user.into(),
            host: host.into(),
            port: port.parse().ok()?,
            ..Default::default()
        }))
    } else {
        let (port, baud) = key.strip_prefix("serial:")?.rsplit_once(':')?;
        Some(SessionKind::Serial(SerialProfile {
            port: port.into(),
            baud: baud.parse().ok()?,
            ..Default::default()
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_ports_and_prompt_modes_are_parsed() {
        for (input, user, pass, host, port, prompt) in [
            ("192.168.1.2", "", None, "192.168.1.2", 22, true),
            ("alice@server", "alice", None, "server", 22, false),
            (
                "alice:se:cr@et@server:2222",
                "alice",
                Some("se:cr@et"),
                "server",
                2222,
                false,
            ),
            (
                ":secret@example.org",
                "",
                Some("secret"),
                "example.org",
                22,
                false,
            ),
            ("example.org", "", None, "example.org", 22, true),
            ("alice@[::1]:2200", "alice", None, "::1", 2200, false),
        ] {
            let target = parse(input).expect(input);
            assert_eq!(
                (
                    &*target.profile.user,
                    target.password.as_ref().map(|p| p.as_str()),
                    &*target.profile.host,
                    target.profile.port,
                    target.prompt
                ),
                (user, pass, host, port, prompt)
            );
        }
    }
    #[test]
    fn invalid_targets_and_search_words_stay_searches() {
        for input in [
            "prod",
            "生产",
            "999.1.2.3",
            "user@",
            "user@bad/host",
            "a@host:0",
            "a@host:65536",
            "https://example.org",
            "foo bar",
            "a@-host",
            "a@host.",
        ] {
            assert!(parse(input).is_none(), "{input}");
        }
    }
}
