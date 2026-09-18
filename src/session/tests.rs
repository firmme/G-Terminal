use super::*;
/// A pane that exists before its connection has to be able to carry the
/// connection's own notices, and it must not be offered as finished.
#[test]
fn a_connecting_pane_has_a_console_and_no_retry_offer() {
    let session = Session::connecting(SessionKind::Local("cmd".into()), 100);
    assert_eq!(session.link(), SessionStatus::Detached);
    assert!(session.status.lock().unwrap().exit_code.is_none());
    session.note("connect to COM3");
    session.note_error("connect failed: busy");
    let text = session.terminal.lock().unwrap().parser.screen().contents();
    assert!(text.contains("connect to COM3"), "{text}");
    assert!(text.contains("connect failed: busy"), "{text}");
}

/// A finished session has no worker left to receive a resize, and that must
/// stay invisible: the status line is what reports the disconnect. The size
/// still has to be recorded, or the same failed send would be repeated on
/// every frame and the raw channel error would surface in the UI.
#[test]
fn resize_after_the_session_ends_is_silent_and_settles() {
    let mut session = Session::disconnected(SessionKind::Local("cmd".into()), 100);
    session.resize(40, 120);
    assert_eq!(session.size, (40, 120));
    session.resize(40, 120);
    assert_eq!(session.size, (40, 120));
}

#[test]
fn shell_labels_are_readable_for_keys_and_paths() {
    assert_eq!(shell_label("powershell"), "PowerShell");
    assert_eq!(shell_label("cmd"), "Command Prompt");
    assert_eq!(shell_label(""), "Shell");
    assert_eq!(shell_label("shell"), "Shell");
    assert_eq!(shell_label("/bin/zsh"), "zsh");
    assert_eq!(shell_label("/opt/homebrew/bin/fish"), "fish");
}

/// Choosing a shell has to start that shell, not silently fall back to
/// `$SHELL` as the pre-enumeration code did.
#[test]
#[cfg(not(windows))]
fn a_chosen_shell_path_is_the_program_that_starts() {
    let command = SessionKind::Local("/bin/bash".into()).command().unwrap();
    assert_eq!(
        command.get_argv()[0],
        std::ffi::OsString::from("/bin/bash"),
        "the chosen shell must be argv[0]"
    );
}

/// The legacy `shell` value and an empty one still mean "follow $SHELL".
#[test]
#[cfg(not(windows))]
fn the_legacy_shell_value_follows_the_environment() {
    let expected =
        std::ffi::OsString::from(std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()));
    for value in ["shell", ""] {
        let command = SessionKind::Local(value.into()).command().unwrap();
        assert_eq!(command.get_argv()[0], expected, "value {value:?}");
    }
}

#[test]
#[cfg(not(windows))]
fn local_shells_are_absolute_unique_and_present_on_disk() {
    let shells = local_shells();
    assert!(
        !shells.is_empty(),
        "a Unix host must offer at least one shell"
    );
    for shell in shells {
        assert!(
            shell.value.starts_with('/'),
            "{} is not absolute",
            shell.value
        );
        assert!(
            std::path::Path::new(&shell.value).is_file(),
            "{} does not exist",
            shell.value
        );
        assert!(!shell.label.is_empty());
    }
    // Names are unique, so the UI never shows two identical rows.
    for (index, shell) in shells.iter().enumerate() {
        assert!(
            !shells[..index]
                .iter()
                .any(|other| other.label == shell.label),
            "duplicate label {}",
            shell.label
        );
    }
}

#[test]
fn a_missing_serial_port_fails_to_spawn_without_panicking() {
    let result = Session::spawn(
        SessionKind::Serial(SerialProfile {
            port: "COM199".into(),
            ..Default::default()
        }),
        100,
        Arc::new(|| {}),
    );
    let error = result.err().expect("COM199 must not exist");
    assert!(format!("{error:#}").contains("COM199"), "{error:#}");
}

#[test]
fn remote_options_cannot_be_injected_through_destination() {
    for host in [
        "-oProxyCommand=calc",
        "host name",
        "a\ncommand",
        "user@host",
    ] {
        let p = RemoteProfile {
            host: host.into(),
            ..Default::default()
        };
        assert!(SessionKind::Ssh(p).command().is_err());
    }
    let p = RemoteProfile {
        host: "::1".into(),
        user: "test".into(),
        ..Default::default()
    };
    assert!(SessionKind::Ssh(p).command().is_ok());
}
