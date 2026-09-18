use super::*;

#[test]
fn links_are_found_only_under_the_pointer() {
    let line = "see https://example.com/a, then http://x.test.";
    let (url, start, end) = link_at(line, 10).expect("inside the first link");
    assert_eq!(url, "https://example.com/a");
    assert_eq!((start, end), (4, 25));
    // A bare `www.` gets a scheme; the pointer has to be inside it.
    let (url, _, _) = link_at("go to www.example.org now", 8).unwrap();
    assert_eq!(url, "http://www.example.org");
    assert!(link_at("go to www.example.org now", 2).is_none());
    // Trailing punctuation is not part of the link.
    let (url, _, _) = link_at("(http://a.test).", 2).unwrap();
    assert_eq!(url, "http://a.test");
}

/// `less` turns on application cursor keys, and then only the SS3 spelling
/// of the arrows moves it — the CSI one scrolls nothing.
#[test]
fn alternate_scroll_respects_the_application_cursor_mode() {
    assert_eq!(alternate_scroll_key(1, false), b"\x1b[A");
    assert_eq!(alternate_scroll_key(-1, false), b"\x1b[B");
    assert_eq!(alternate_scroll_key(1, true), b"\x1bOA");
    assert_eq!(alternate_scroll_key(-1, true), b"\x1bOB");
}

#[test]
fn drag_selection_can_copy_automatically_without_touching_system_clipboard() {
    let ctx = egui::Context::default();
    let session = Session::disconnected(g_terminal::session::SessionKind::Local("cmd".into()), 100);
    session.terminal.lock().unwrap().process(b"hello world");
    let mut pane = Pane::new(900, session);
    let mut copy = String::new();
    let frames = vec![
        vec![],
        vec![
            Event::PointerMoved(egui::pos2(16.0, 20.0)),
            Event::PointerButton {
                pos: egui::pos2(16.0, 20.0),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
        vec![Event::PointerMoved(egui::pos2(75.0, 20.0))],
        vec![Event::PointerButton {
            pos: egui::pos2(75.0, 20.0),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    ];
    for events in frames {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 300.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    pane.show(ui, true_options());
                });
            },
        );
        for command in output.platform_output.commands {
            if let egui::OutputCommand::CopyText(text) = command {
                copy = text;
            }
        }
    }
    assert!(!copy.is_empty());
    assert!("hello world".contains(&copy));
}
fn true_options() -> ViewOptions<'static> {
    ViewOptions {
        active: true,
        keyboard_enabled: true,
        size: 15.0,
        palette: Palette::new(false),
        query: "",
        copy_on_select: true,
        search_engine: "google",
    }
}
#[test]
fn selection_copies_wide_cells_once_and_preserves_wrapping() {
    let mut p = vt100::Parser::new(3, 6, 0);
    p.process("你好abcdef".as_bytes());
    assert_eq!(selection_text(p.screen(), (0, 0), (1, 3)), "你好abcdef");
    assert_eq!(selection_text(p.screen(), (1, 3), (0, 0)), "你好abcdef");
}

/// One click places the caret, two take a word, three take the line, four take
/// the screen — and leaning on the button stays at "everything" rather than
/// wrapping round to something else.
#[test]
fn click_runs_map_to_the_expected_gesture() {
    assert_eq!(click_gesture(0), Gesture::Point);
    assert_eq!(click_gesture(1), Gesture::Point);
    assert_eq!(click_gesture(2), Gesture::Word);
    assert_eq!(click_gesture(3), Gesture::Line);
    assert_eq!(click_gesture(4), Gesture::Screen);
    assert_eq!(click_gesture(5), Gesture::Screen);
    assert_eq!(click_gesture(u8::MAX), Gesture::Screen);
}

/// A run only continues on the same cell, soon enough after the last click.
/// Getting this wrong would turn two separate clicks into a word selection.
#[test]
fn a_click_run_needs_the_same_cell_and_a_short_gap() {
    let here = (4, 10);
    assert!(continues_run(here, here, 0.05, 0.3));
    assert!(continues_run(here, here, 0.3, 0.3), "the boundary counts");
    assert!(!continues_run(here, here, 0.31, 0.3), "too slow");
    assert!(!continues_run(here, (4, 11), 0.05, 0.3), "another cell");
    assert!(!continues_run(here, (5, 10), 0.05, 0.3), "another row");
    // A clock that went backwards must not merge two clicks either.
    assert!(!continues_run(here, here, -1.0, 0.3));
}
