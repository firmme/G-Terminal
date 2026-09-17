#![cfg(windows)]

use g_terminal::session::{Session, SessionKind};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn wait_until(session: &Session, timeout: Duration, predicate: impl Fn(&vt100::Screen) -> bool) {
    let started = Instant::now();
    loop {
        if predicate(session.terminal.lock().unwrap().parser.screen()) {
            return;
        }
        assert!(
            started.elapsed() < timeout,
            "PTY timeout. Screen: {:?}; status: {:?}",
            session.terminal.lock().unwrap().parser.screen().contents(),
            session.status.lock().unwrap()
        );
        std::thread::sleep(Duration::from_millis(40));
    }
}

#[test]
fn real_powershell_roundtrip_resize_unicode_and_exit() {
    let mut session = Session::spawn(
        SessionKind::Local("powershell".into()),
        100,
        Arc::new(|| {}),
    )
    .unwrap();
    wait_until(&session, Duration::from_secs(15), |s| {
        s.contents().contains("PS ")
    });
    session.resize(35, 110);
    wait_until(&session, Duration::from_secs(5), |s| s.size() == (35, 110));
    session.write("Write-Output ('ROUND' + 'TRIP_OK'); Write-Output ([char]0x4F60 + [string][char]0x597D)\r".as_bytes().to_vec()).unwrap();
    wait_until(&session, Duration::from_secs(10), |s| {
        s.contents().contains("ROUNDTRIP_OK") && s.contents().contains("你好")
    });
    session.write(b"exit 7\r".to_vec()).unwrap();
    let start = Instant::now();
    while session.status.lock().unwrap().exit_code.is_none() {
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(session.status.lock().unwrap().exit_code, Some(7));
}

#[test]
fn real_cmd_accepts_input_and_conpty_closes_without_blocking() {
    let session = Session::spawn(SessionKind::Local("cmd".into()), 100, Arc::new(|| {})).unwrap();
    wait_until(&session, Duration::from_secs(10), |s| {
        s.contents().contains('>')
    });
    session.write(b"echo %COMSPEC%\r".to_vec()).unwrap();
    wait_until(&session, Duration::from_secs(10), |s| {
        s.contents()
            .to_ascii_lowercase()
            .contains("system32\\cmd.exe")
    });
    let status = session.status.clone();
    let before_drop = Instant::now();
    drop(session);
    assert!(
        before_drop.elapsed() < Duration::from_secs(1),
        "Closing a tab blocked the caller"
    );
    let start = Instant::now();
    loop {
        let state = status.lock().unwrap().clone();
        if state.exit_code.is_some() && state.eof {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "PTY worker did not shut down: {state:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
