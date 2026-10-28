//! README captures use only synthetic data and never start a shell or connect.
use super::*;

pub(super) fn settings() -> Settings {
    Settings {
        groups: vec!["演示环境".into(), "设备".into()],
        profiles: vec![
            RemoteProfile {
                name: "Web Demo".into(),
                host: "192.0.2.10".into(),
                user: "demo".into(),
                group: "演示环境".into(),
                color: "#37bfa7".into(),
                ..Default::default()
            },
            RemoteProfile {
                name: "Database Demo".into(),
                host: "192.0.2.20".into(),
                user: "demo".into(),
                group: "演示环境".into(),
                ..Default::default()
            },
        ],
        serial_profiles: vec![SerialProfile {
            name: "Console Demo".into(),
            port: "COM3".into(),
            group: "设备".into(),
            ..Default::default()
        }],
        restore_tabs: false,
        ..Default::default()
    }
}

impl App {
    pub(super) fn populate_screenshot_demo(&mut self) {
        let session = Session::disconnected(
            SessionKind::Local(self.settings.default_shell.clone()),
            self.settings.scrollback,
        );
        session.terminal.locked().process(
            concat!(
                "\x1b[2J\x1b[H\x1b[1;36mG-Terminal\x1b[0m  /  Native GPU terminal\r\n\r\n",
                "\x1b[90mDemo workspace - sample output\x1b[0m\r\n\r\n",
                "\x1b[32mPS C:\\Demo>\x1b[0m cargo build --release\r\n",
                "   \x1b[32mCompiling\x1b[0m g-terminal v",
                env!("CARGO_PKG_VERSION"),
                "\r\n",
                "    \x1b[32mFinished\x1b[0m release profile [optimized]\r\n\r\n",
                "\x1b[32mPS C:\\Demo>\x1b[0m ls\r\n\r\n",
                "    docs/    src/    Cargo.toml    README.md\r\n\r\n",
                "\x1b[32mPS C:\\Demo>\x1b[0m\r\n"
            )
            .as_bytes(),
        );
        self.next_id += 1;
        self.tabs.push(Tab {
            id: self.next_id,
            panes: vec![Pane::new(self.next_id, session)],
            focused: 0,
            layout: PaneLayout::Leaf(0),
            seen_output: 0,
        });
        if std::env::var("GTERMINAL_SCREENSHOT_VIEW").as_deref() == Ok("activity") {
            let session = Session::disconnected(
                SessionKind::Ssh(self.settings.profiles[0].clone()),
                self.settings.scrollback,
            );
            session
                .traffic
                .down
                .store(128, std::sync::atomic::Ordering::Relaxed);
            self.next_id += 1;
            self.tabs.push(Tab {
                id: self.next_id,
                panes: vec![Pane::new(self.next_id, session)],
                focused: 0,
                layout: PaneLayout::Leaf(0),
                seen_output: 0,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_uses_only_sample_connections_without_credentials_or_workspace() {
        let settings = settings();
        assert!(settings.ssh_config_profiles.is_empty());
        assert!(settings.workspace.is_empty());
        assert!(!settings.onedrive_enabled);
        assert!(!settings.restore_tabs);
        for profile in &settings.profiles {
            assert!(profile.host.starts_with("192.0.2."));
            assert_eq!(profile.user, "demo");
            assert!(profile.identity.is_empty());
            assert!(profile.jump.is_none());
            assert!(profile.forwards.is_empty());
        }
    }
}
