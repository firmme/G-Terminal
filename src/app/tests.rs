use super::*;
use egui::{Event, Modifiers, RawInput, Rect, Vec2};

#[test]
fn nested_workspace_roundtrips_without_persisting_authentication() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    app.execute(Action::Split(Axis::Horizontal), &ctx);
    app.execute(Action::Split(Axis::Vertical), &ctx);
    assert_eq!(app.tabs[0].panes.len(), 3);
    assert!(app.tabs[0].layout.valid(3));
    app.snapshot();
    let config = serde_json::to_string(&app.settings).unwrap();
    assert!(!config.contains("password"));
    let settings: Settings = serde_json::from_str(&config).unwrap();
    drop(app);
    let restored = App::from_settings(&ctx, settings, None, None);
    assert_eq!(restored.tabs[0].panes.len(), 3);
    assert!(restored.tabs[0].layout.valid(3));
}

fn frame(app: &mut App, ctx: &egui::Context, events: Vec<Event>, modifiers: Modifiers, size: Vec2) {
    let _ = ctx.run(
        RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
            modifiers,
            events,
            ..Default::default()
        },
        |ctx| app.render(ctx),
    );
}

/// Pumps frames until a pending serial owner check has answered and the connect
/// it guards has settled. The check runs on a worker (it opens the device, and
/// on some platforms walks system handles), so a test cannot assume it is done
/// by the time `connect_pane` returns.
fn settle_serial(app: &mut App, ctx: &egui::Context, size: Vec2) {
    for _ in 0..400 {
        app.poll_serial_probe(ctx);
        if app.serial_probe.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        frame(app, ctx, vec![], Modifiers::NONE, size);
    }
    frame(app, ctx, vec![], Modifiers::NONE, size);
}

fn key(key: Key, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

#[test]
fn workspace_routes_keyboard_splits_and_dialogs_without_leaking_commands() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    let size = Vec2::new(1280.0, 800.0);
    // `command` is what the accelerators are written against: it is Ctrl
    // on Windows and Cmd on macOS, and egui-winit sets both for Ctrl.
    let mods = Modifiers {
        ctrl: true,
        command: true,
        shift: true,
        ..Default::default()
    };
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    frame(&mut app, &ctx, vec![key(Key::T, mods)], mods, size);
    assert_eq!(app.tabs.len(), 2);
    frame(&mut app, &ctx, vec![key(Key::D, mods)], mods, size);
    assert_eq!(app.tabs[1].panes.len(), 2);
    assert_eq!(app.tabs[1].focused, 1);
    frame(
        &mut app,
        &ctx,
        vec![],
        Modifiers::NONE,
        Vec2::new(760.0, 480.0),
    );
    let start = std::time::Instant::now();
    while !app.tabs[1].panes[1]
        .session
        .terminal
        .lock()
        .unwrap()
        .parser
        .screen()
        .contents()
        .contains("PS ")
    {
        assert!(start.elapsed().as_secs() < 15);
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    frame(
        &mut app,
        &ctx,
        vec![
            Event::Text("Write-Output ('GUI_' + 'INPUT_OK')".into()),
            key(Key::Enter, Modifiers::NONE),
        ],
        Modifiers::NONE,
        size,
    );
    let start = std::time::Instant::now();
    while !app.tabs[1].panes[1]
        .session
        .terminal
        .lock()
        .unwrap()
        .parser
        .screen()
        .contents()
        .contains("GUI_INPUT_OK")
    {
        assert!(
            start.elapsed().as_secs() < 10,
            "Keyboard did not reach active pane"
        );
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    assert!(
        !app.tabs[1].panes[0]
            .session
            .terminal
            .lock()
            .unwrap()
            .parser
            .screen()
            .contents()
            .contains("GUI_INPUT_OK")
    );
    frame(&mut app, &ctx, vec![key(Key::F, mods)], mods, size);
    assert!(app.search_open);
    frame(
        &mut app,
        &ctx,
        vec![key(Key::Escape, Modifiers::NONE)],
        Modifiers::NONE,
        size,
    );
    assert!(!app.search_open);
    frame(&mut app, &ctx, vec![key(Key::W, mods)], mods, size);
    assert_eq!(app.tabs[1].panes.len(), 1);
    frame(&mut app, &ctx, vec![key(Key::W, mods)], mods, size);
    assert_eq!(app.tabs.len(), 1);
    assert!(app.error.is_none(), "{:?}", app.error);
}

/// The wheel scrolls the pane's own history, and a restart keeps that
/// history by reusing the same screen. What the reused screen *contains* is
/// `terminal`'s business and is covered there; here the point is the wiring,
/// which is why the assertion is on the screen's identity rather than on
/// output that a dying shell could still race with.
#[test]
fn wheel_scrolls_history_and_restart_keeps_it() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    let size = Vec2::new(1280.0, 800.0);
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    {
        let mut terminal = app.tabs[0].panes[0].session.terminal.lock().unwrap();
        for i in 0..80 {
            terminal.process(format!("history {i}\r\n").as_bytes());
        }
    }
    let center = egui::Pos2::new(700.0, 400.0);
    frame(
        &mut app,
        &ctx,
        vec![Event::PointerMoved(center)],
        Modifiers::NONE,
        size,
    );
    for _ in 0..20 {
        frame(
            &mut app,
            &ctx,
            vec![Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: Vec2::new(0.0, 1.0),
                modifiers: Modifiers::NONE,
            }],
            Modifiers::NONE,
            size,
        );
    }
    let scrolled = app.tabs[0].panes[0]
        .session
        .terminal
        .lock()
        .unwrap()
        .parser
        .screen()
        .scrollback();
    assert!(scrolled > 0, "the wheel did not move the history");

    let before = app.tabs[0].panes[0].session.terminal.clone();
    app.execute(Action::Restart, &ctx);
    assert!(app.error.is_none(), "{:?}", app.error);
    let after = app.tabs[0].panes[0].session.terminal.clone();
    assert!(
        Arc::ptr_eq(&before, &after),
        "the restart did not carry the screen over"
    );
}

/// The spinner has to actually paint: a bare panel so the only shapes are
/// the eight squares of the ring.
#[test]
fn the_spinner_paints_its_whole_ring() {
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| connecting_spinner(ui, Palette::new(false)));
    });
    assert!(
        output.shapes.len() >= 8,
        "the spinner painted {} shapes",
        output.shapes.len()
    );
}

/// The spinner is driven by panes that have no connection yet, and it has
/// to stop once the attempt settles either way.
#[test]
fn the_spinner_runs_only_while_a_connection_is_pending() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    let size = Vec2::new(1280.0, 800.0);
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    assert!(!app.connecting());
    let serial = SessionKind::Serial(SerialProfile {
        port: "COM199".into(),
        ..Default::default()
    });
    let (tab_id, pane_id) = app.open_connecting_tab(serial.clone());
    assert!(app.connecting(), "a pending pane must spin");
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    app.connect_pane(tab_id, pane_id, serial, &ctx);
    settle_serial(&mut app, &ctx, size);
    assert!(!app.connecting(), "a settled attempt must stop the spinner");
    assert!(app.serial_probe.is_none(), "the probe must be consumed");
}

/// The arrows light on a burst and settle back to idle once it stops.
#[test]
fn the_rate_meter_lights_up_then_settles() {
    let mut meter = RateMeter::default();
    assert!(
        !meter.sample(1, 0, 0),
        "the first sample is only a baseline"
    );
    assert!(meter.sample(1, 4096, 0), "a write must light the up arrow");
    let mut settled = None;
    for frame in 0..64 {
        if !meter.sample(1, 4096, 0) {
            settled = Some(frame);
            break;
        }
    }
    assert!(settled.is_some(), "the rate never decayed to idle");
    assert!(meter.up_per_sec <= TRAFFIC_IDLE);
    assert_eq!(meter.down_per_sec, 0.0);
}

/// A background tab that receives output is underlined, and looking at it
/// clears the mark.
#[test]
fn a_background_tab_is_marked_until_it_is_looked_at() {
    use std::sync::atomic::Ordering;
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    let size = Vec2::new(1280.0, 800.0);
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    app.open_connecting_tab(SessionKind::Serial(SerialProfile {
        port: "COM199".into(),
        ..Default::default()
    }));
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.active, 1, "the new tab is the active one");
    assert!(!tab_updated(&app.tabs[1], true), "the active tab is clean");

    // The first tab reads something while it is off screen.
    app.tabs[0].panes[0]
        .session
        .traffic
        .down
        .fetch_add(64, Ordering::Relaxed);
    assert!(tab_updated(&app.tabs[0], false), "new output must mark it");

    app.active = 0;
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    assert!(
        !tab_updated(&app.tabs[0], true),
        "looking at the tab must clear the mark"
    );
    assert_eq!(app.tabs[0].seen_output, tab_output(&app.tabs[0]));
}

/// Closes run one frame and report whether a Close command was sent.
fn closes(ctx: &egui::Context, app: &mut App, size: Vec2) -> bool {
    let output = ctx.run(
        RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        },
        |ctx| app.request_exit(ctx),
    );
    output
        .viewport_output
        .into_values()
        .flat_map(|viewport| viewport.commands)
        .any(|command| matches!(command, egui::ViewportCommand::Close))
}

/// With a live session the close is held back and the dialog is armed; only
/// a confirmation closes the window. This is what the titlebar button and a
/// platform close request both go through.
#[test]
fn closing_asks_first_while_a_session_is_live() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    let size = Vec2::new(1280.0, 800.0);
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    app.settings.confirm_on_exit = true;
    assert!(app.has_live_sessions(), "the startup shell is live");

    assert!(!closes(&ctx, &mut app, size), "the close must be held back");
    assert!(app.confirm_exit, "the dialog must be armed");

    app.exit_confirmed = true;
    assert!(closes(&ctx, &mut app, size), "confirming must close");
}

#[test]
fn closing_does_not_ask_when_nothing_is_connected() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    let size = Vec2::new(1280.0, 800.0);
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    app.settings.confirm_on_exit = true;
    app.execute(Action::Disconnect, &ctx);
    assert!(!app.has_live_sessions());
    assert!(closes(&ctx, &mut app, size), "nothing live means it closes");
    assert!(!app.confirm_exit, "no dialog for a detached session");
}

/// Enter commits an IME composition, and egui does not filter it out, so a
/// form saving on Enter used to save while the user was still picking a
/// pinyin candidate. The save itself is what the test watches: an invalid
/// profile puts a message in the status bar.
#[test]
fn enter_does_not_save_a_form_while_a_candidate_list_is_open() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    let size = Vec2::new(1280.0, 800.0);
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    app.execute(Action::Remote, &ctx);
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);

    // Composing, then Enter to commit it: the form must stay put.
    frame(
        &mut app,
        &ctx,
        vec![
            Event::Ime(egui::ImeEvent::Preedit("lianjie".into())),
            key(Key::Enter, Modifiers::NONE),
        ],
        Modifiers::NONE,
        size,
    );
    assert!(
        app.error.is_none(),
        "Enter while composing ran the save: {:?}",
        app.error
    );
    assert!(app.remote_open, "the form must still be open");

    // With the composition committed, Enter saves as usual.
    frame(
        &mut app,
        &ctx,
        vec![Event::Ime(egui::ImeEvent::Commit("连接".into()))],
        Modifiers::NONE,
        size,
    );
    app.remote.host = "example.com".into();
    frame(
        &mut app,
        &ctx,
        vec![key(Key::Enter, Modifiers::NONE)],
        Modifiers::NONE,
        size,
    );
    assert!(!app.remote_open, "a plain Enter still saves");
}

/// The toolbox types Linux commands, so it must not open on a local shell.
#[test]
fn the_toolbox_refuses_a_session_that_is_not_ssh() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    let size = Vec2::new(1280.0, 800.0);
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    app.execute(Action::Toolbox, &ctx);
    assert!(app.toolbox.is_none());
    assert_eq!(
        app.error.as_deref(),
        Some("服务器工具箱仅用于已连接的 SSH 会话")
    );
}

#[test]
fn bitrates_use_decimal_units() {
    assert_eq!(format_bitrate(0.0), "0 bps");
    assert_eq!(format_bitrate(1.0), "8 bps");
    assert_eq!(format_bitrate(125.0), "1.0 Kbps");
    assert_eq!(format_bitrate(1_000_000.0), "8.0 Mbps");
}

/// Alt+C drops the session but keeps the pane and its screen, so Alt+R has
/// something to bring back.
#[test]
fn the_disconnect_shortcut_keeps_the_pane_and_reconnects() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    let size = Vec2::new(1280.0, 800.0);
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    let before = app.tabs[0].panes[0].session.terminal.clone();
    let alt = Modifiers {
        alt: true,
        ..Default::default()
    };
    frame(&mut app, &ctx, vec![key(Key::C, alt)], alt, size);
    assert_eq!(app.tabs.len(), 1, "断开 must not close the pane");
    let session = &app.tabs[0].panes[0].session;
    assert_eq!(session.link(), SessionStatus::Detached);
    assert!(
        Arc::ptr_eq(&before, &session.terminal),
        "断开 must keep the screen"
    );
    assert!(
        session
            .terminal
            .lock()
            .unwrap()
            .parser
            .screen()
            .contents()
            .contains("已断开")
    );

    frame(&mut app, &ctx, vec![key(Key::R, alt)], alt, size);
    assert!(app.error.is_none(), "{:?}", app.error);
    assert_eq!(app.tabs[0].panes[0].session.link(), SessionStatus::Live);
}

/// Alt+R arrives as a key event *and* as text. The key is the shortcut; the
/// text must not be typed into the session — on an ended session that was
/// reported as a spurious write error right after reconnecting.
#[test]
fn a_shortcut_does_not_leak_its_text_into_the_session() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    let size = Vec2::new(1280.0, 800.0);
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    app.execute(Action::Disconnect, &ctx);
    assert_eq!(app.tabs[0].panes[0].session.link(), SessionStatus::Detached);

    let alt = Modifiers {
        alt: true,
        ..Default::default()
    };
    frame(
        &mut app,
        &ctx,
        vec![key(Key::R, alt), Event::Text("r".into())],
        alt,
        size,
    );
    assert!(
        app.error.is_none(),
        "the shortcut's text reached the ended session: {:?}",
        app.error
    );
    assert_eq!(
        app.tabs[0].panes[0].session.link(),
        SessionStatus::Live,
        "Alt+R must still reconnect"
    );
}

#[test]
fn a_toast_stays_for_its_lifetime_then_goes() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    let size = Vec2::new(1280.0, 800.0);
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    app.notify("已复制 3 个字符");
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    assert!(app.toast.is_some(), "an unexpired toast must stay");
    app.toast = Some((
        "old".into(),
        std::time::Instant::now() - TOAST_LIFETIME - std::time::Duration::from_secs(1),
    ));
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    assert!(app.toast.is_none(), "an expired toast must be dropped");
}

/// A connection gets its tab before it is attempted, and a failure lands in
/// that tab's own console instead of a status line shared by every session.
/// COM199 is used because it cannot exist, so no device is touched.
#[test]
fn a_failed_connection_reports_in_its_own_console() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    let size = Vec2::new(1280.0, 800.0);
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    let before = app.tabs.len();
    app.execute(
        Action::New(SessionKind::Serial(SerialProfile {
            port: "COM199".into(),
            ..Default::default()
        })),
        &ctx,
    );
    assert_eq!(
        app.tabs.len(),
        before + 1,
        "the tab must exist before the connect attempt"
    );
    settle_serial(&mut app, &ctx, size);
    let terminal = app.tabs.last().unwrap().panes[0].session.terminal.clone();
    let text = terminal.lock().unwrap().parser.screen().contents();
    assert!(text.contains("connect to COM199"), "{text}");
    assert!(text.contains("connect failed"), "{text}");
    assert!(
        app.error.is_none(),
        "a connection failure belongs to its own console: {:?}",
        app.error
    );
}

/// The console offers its clear actions on right-click, and the menu has to
/// survive more than the frame that opened it.
#[test]
fn right_click_opens_the_console_menu_and_it_stays_open() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    let size = Vec2::new(1280.0, 800.0);
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    let pos = egui::Pos2::new(700.0, 400.0);
    frame(
        &mut app,
        &ctx,
        vec![Event::PointerMoved(pos)],
        Modifiers::NONE,
        size,
    );
    for pressed in [true, false] {
        frame(
            &mut app,
            &ctx,
            vec![Event::PointerButton {
                pos,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: Modifiers::NONE,
            }],
            Modifiers::NONE,
            size,
        );
    }
    assert!(egui::Popup::is_any_open(&ctx), "the menu did not open");
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    assert!(
        egui::Popup::is_any_open(&ctx),
        "the menu closed on the next frame"
    );
}

/// The serial picker and the serial tab of the connection form only ever
/// list devices; opening a port happens on connect. A port that is not
/// attached is used here precisely so the test cannot touch real hardware.
#[test]
fn serial_picker_and_form_open_without_touching_a_device() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    let size = Vec2::new(1280.0, 800.0);
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);

    app.execute(Action::SerialPicker, &ctx);
    assert!(app.serial_picker.is_some());
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);

    app.execute(Action::Remote, &ctx);
    app.profile_kind = ProfileKind::Serial;
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    assert!(app.remote_open);

    app.settings.serial_profiles.push(SerialProfile {
        port: "COM199".into(),
        ..Default::default()
    });
    app.execute(Action::EditSerial(0), &ctx);
    assert_eq!(app.profile_kind, ProfileKind::Serial);
    assert_eq!(app.serial.port, "COM199");
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);

    app.execute(Action::RemoveSerial(0), &ctx);
    assert!(app.settings.serial_profiles.is_empty());
    assert!(app.error.is_none(), "{:?}", app.error);
}

/// The auto-hidden bar is out of the layout until the pointer reaches the
/// left strip, and collapses again when it leaves. The reveal state is the
/// whole interaction, so it is asserted directly.
#[test]
fn auto_hidden_sidebar_reveals_on_the_left_strip() {
    let ctx = egui::Context::default();
    let settings = Settings {
        auto_hide_sidebar: true,
        ..Settings::default()
    };
    let mut app = App::from_settings(&ctx, settings, None, None);
    let size = Vec2::new(1280.0, 800.0);
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    assert!(!app.sidebar_reveal, "the bar must start hidden");
    frame(
        &mut app,
        &ctx,
        vec![Event::PointerMoved(egui::pos2(3.0, 400.0))],
        Modifiers::NONE,
        size,
    );
    assert!(app.sidebar_reveal, "the left strip must reveal the bar");
    frame(
        &mut app,
        &ctx,
        vec![Event::PointerMoved(egui::pos2(900.0, 400.0))],
        Modifiers::NONE,
        size,
    );
    assert!(
        !app.sidebar_reveal,
        "leaving the bar and the strip must collapse it"
    );
}

/// Runs one frame and collects the viewport commands it produced, so window
/// chrome behaviour can be asserted instead of eyeballed.
fn commands(
    app: &mut App,
    ctx: &egui::Context,
    events: Vec<Event>,
    size: Vec2,
) -> Vec<egui::ViewportCommand> {
    let output = ctx.run(
        RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        },
        |ctx| app.render(ctx),
    );
    output
        .viewport_output
        .into_values()
        .flat_map(|viewport| viewport.commands)
        .collect()
}

fn press(pos: egui::Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// The merged bar has to keep dragging the frameless window by its empty area.
/// That is the whole reason the tab row and the window buttons can share one
/// panel, so it is worth asserting rather than assuming.
#[test]
fn dragging_the_empty_bar_starts_a_window_drag() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    let size = Vec2::new(1280.0, 800.0);
    // The drag rect must exist for a frame before egui can hit-test against it.
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    // Well clear of the menu, the tabs and the three window buttons.
    let empty = egui::Pos2::new(640.0, 13.0);
    frame(
        &mut app,
        &ctx,
        vec![Event::PointerMoved(empty), press(empty, true)],
        Modifiers::NONE,
        size,
    );
    let mut seen = Vec::new();
    for step in 1..=3 {
        seen.extend(commands(
            &mut app,
            &ctx,
            vec![Event::PointerMoved(
                empty + Vec2::new(30.0 * step as f32, 0.0),
            )],
            size,
        ));
    }
    assert!(
        seen.contains(&egui::ViewportCommand::StartDrag),
        "the empty bar no longer drags the window: {seen:?}"
    );
}

/// The drag strip is registered after the buttons, so it wins any overlap in
/// hit-testing. This pins the boundary that keeps it off their clicks.
#[test]
fn window_buttons_are_not_swallowed_by_the_drag_strip() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    let size = Vec2::new(1280.0, 800.0);
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    // Right to left from the panel's 4pt inner margin: close, then maximize.
    let maximize = egui::Pos2::new(size.x - 4.0 - 32.0 - 4.0 - 16.0, 13.0);
    frame(
        &mut app,
        &ctx,
        vec![Event::PointerMoved(maximize), press(maximize, true)],
        Modifiers::NONE,
        size,
    );
    let seen = commands(&mut app, &ctx, vec![press(maximize, false)], size);
    assert!(
        seen.iter()
            .any(|c| matches!(c, egui::ViewportCommand::Maximized(_))),
        "the maximize button produced {seen:?}"
    );
    assert!(
        !seen.contains(&egui::ViewportCommand::StartDrag),
        "the drag strip swallowed the button click"
    );
}

/// A frameless window gets no resize border from the OS, and winit does not
/// add one, so the edges are claimed in-app. This pins that a press on the
/// border asks the platform for a resize — and that a press away from every
/// border does not, or ordinary clicking would start dragging the frame.
#[test]
fn pressing_a_window_edge_asks_the_platform_to_resize() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    let size = Vec2::new(1280.0, 800.0);
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);

    // Just inside the left edge.
    let edge = egui::Pos2::new(2.0, 400.0);
    let seen = commands(
        &mut app,
        &ctx,
        vec![Event::PointerMoved(edge), press(edge, true)],
        size,
    );
    assert!(
        seen.iter()
            .any(|command| matches!(command, egui::ViewportCommand::BeginResize(_))),
        "the window edge did not start a resize: {seen:?}"
    );

    // Well clear of every edge.
    let middle = egui::Pos2::new(640.0, 400.0);
    let seen = commands(
        &mut app,
        &ctx,
        vec![Event::PointerMoved(middle), press(middle, false)],
        size,
    );
    assert!(
        !seen
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::BeginResize(_))),
        "a click in the middle started a resize: {seen:?}"
    );
}
