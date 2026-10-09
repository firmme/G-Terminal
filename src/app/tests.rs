use super::*;
use egui::{Event, Modifiers, RawInput, Rect, Vec2};

#[test]
fn connection_dialog_keeps_fields_and_footer_visible() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    app.remote_open = true;
    let input = || RawInput {
        screen_rect: Some(Rect::from_min_size(
            egui::Pos2::ZERO,
            Vec2::new(1280.0, 800.0),
        )),
        ..Default::default()
    };
    for _ in 0..12 {
        let _ = ctx.run(input(), |ctx| app.render(ctx));
    }
    let output = ctx.run(input(), |ctx| app.render(ctx));
    let bounds = egui::AreaState::load(&ctx, egui::Id::new("连接配置"))
        .unwrap()
        .rect();
    assert!(
        bounds.width() <= 500.0 && bounds.height() <= 600.0,
        "the connection form should stay compact: {bounds:?}"
    );
    for expected in ["主机 / IP", "用户名", "保存并连接", "取消"] {
        assert!(output.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == expected && shape.clip_rect.contains(text.pos))
        }), "{expected} is clipped: {:?}", output.shapes.iter().filter_map(|shape| if let egui::Shape::Text(text) = &shape.shape { Some((text.galley.text(), text.pos, shape.clip_rect)) } else { None }).collect::<Vec<_>>());
    }
}

#[test]
fn settings_and_confirmation_fit_their_content() {
    for (title, confirmation, max_width, max_height) in [
        ("偏好设置", false, 640.0, 600.0),
        ("确认退出", true, 380.0, 200.0),
    ] {
        let ctx = egui::Context::default();
        let mut app = App::from_settings(&ctx, Settings::default(), None, None);
        app.settings_open = !confirmation;
        app.confirm_exit = confirmation;
        for _ in 0..12 {
            frame(
                &mut app,
                &ctx,
                vec![],
                Modifiers::NONE,
                Vec2::new(1280.0, 800.0),
            );
        }
        let bounds = egui::AreaState::load(&ctx, egui::Id::new(title))
            .unwrap()
            .rect();
        assert!(
            bounds.width() <= max_width && bounds.height() <= max_height,
            "{title} is oversized: {bounds:?}"
        );
    }
}

#[test]
fn login_opens_on_username_and_allows_tabbing_to_password() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    let size = Vec2::new(1280.0, 800.0);
    app.execute(
        Action::QuickConnect(quick_connect::parse("192.0.2.1").unwrap()),
        &ctx,
    );
    for _ in 0..3 {
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    }
    let username = ctx.memory(|memory| memory.focused()).unwrap();
    frame(
        &mut app,
        &ctx,
        vec![Event::Text("alice".into())],
        Modifiers::NONE,
        size,
    );
    assert!(app.login.as_ref().unwrap().profile.user.ends_with("alice"));
    frame(
        &mut app,
        &ctx,
        vec![Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
        Modifiers::NONE,
        size,
    );
    let password = ctx.memory(|memory| memory.focused()).unwrap();
    assert_ne!(username, password);
    let user = app.login.as_ref().unwrap().profile.user.clone();
    for _ in 0..3 {
        frame(
            &mut app,
            &ctx,
            vec![Event::Text("secret".into())],
            Modifiers::NONE,
            size,
        );
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(password));
        assert_eq!(app.login.as_ref().unwrap().profile.user, user);
    }
}

#[test]
fn connection_forms_choose_required_fields_and_refocus_on_reopen() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    let size = Vec2::new(1280.0, 800.0);
    app.remote_open = true;
    app.remote.host.clear();
    for _ in 0..3 {
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    }
    frame(
        &mut app,
        &ctx,
        vec![Event::Text("example.org".into())],
        Modifiers::NONE,
        size,
    );
    assert_eq!(app.remote.host, "example.org");
    app.remote_open = false;
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    app.remote_open = true;
    for _ in 0..3 {
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    }
    frame(
        &mut app,
        &ctx,
        vec![Event::Text("alice".into())],
        Modifiers::NONE,
        size,
    );
    assert!(app.remote.user.ends_with("alice"));
    assert_eq!(app.remote.host, "example.org");
    app.remote_open = false;
    app.execute(Action::SerialPicker, &ctx);
    app.serial_picker.as_mut().unwrap().port.clear();
    for _ in 0..3 {
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    }
    frame(
        &mut app,
        &ctx,
        vec![Event::Text("COM42".into())],
        Modifiers::NONE,
        size,
    );
    assert_eq!(app.serial_picker.as_ref().unwrap().port, "COM42");
}

#[test]
fn copied_connection_text_contains_targets_but_no_key_paths() {
    let remote = RemoteProfile {
        name: "生产机".into(),
        host: "example.org".into(),
        user: "alice".into(),
        port: 2222,
        identity: "private/key/path".into(),
        group: "服务器".into(),
        ..RemoteProfile::default()
    };
    let serial = SerialProfile {
        name: "设备".into(),
        port: "COM9".into(),
        baud: 57600,
        ..SerialProfile::default()
    };
    let settings = Settings {
        profiles: vec![remote.clone()],
        serial_profiles: vec![serial],
        ..Settings::default()
    };
    let list = connection_list_info(&settings);
    assert!(list.contains("SSH | 生产机 | alice@example.org:2222"));
    assert!(list.contains("串口 | 设备 | COM9 | 57600 bps"));
    assert!(!list.contains("private/key/path"));
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, settings, None, None);
    let tab = tab_info(&app.tabs[app.active]);
    assert!(tab.contains("选项卡 |"));
    assert!(tab.contains("窗格 1（当前） |"));
    let output = ctx.run(RawInput::default(), |ctx| {
        app.execute(Action::CopyConnectionList, ctx);
    });
    assert!(output.platform_output.commands.iter().any(|command| {
        matches!(command, egui::OutputCommand::CopyText(text) if text == &list)
    }));
}

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

#[test]
fn hovering_inactive_tabs_keeps_the_strip_geometry_stable() {
    for light_theme in [false, true] {
        for tagged in [false, true] {
            let ctx = egui::Context::default();
            let mut app = App::from_settings(
                &ctx,
                Settings {
                    light_theme,
                    ..Settings::default()
                },
                None,
                None,
            );
            app.execute(
                Action::New(SessionKind::Local(app.settings.default_shell.clone())),
                &ctx,
            );
            if tagged {
                app.tabs[0].panes[0].session.kind = SessionKind::Ssh(RemoteProfile {
                    host: "example.org".into(),
                    color: "#e7a45e".into(),
                    ..Default::default()
                });
            }
            let mut render = |events| {
                let output = ctx.run(
                    RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            egui::Pos2::ZERO,
                            Vec2::new(1280.0, 800.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ctx| app.topbar(ctx, &mut None),
                );
                output
                    .shapes
                    .iter()
                    .filter_map(|shape| {
                        if let egui::epaint::Shape::Rect(rect) = &shape.shape
                            && rect.corner_radius.nw == 7
                            && rect.corner_radius.sw == 0
                        {
                            Some(rect.rect)
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
            };
            for _ in 0..3 {
                render(vec![]);
            }
            let baseline = render(vec![]);
            assert_eq!(baseline.len(), 2);
            let pointer = baseline[0].center();
            // Hover feedback reads the prior frame's response, so check several frames.
            for _ in 0..4 {
                assert_eq!(
                    render(vec![Event::PointerMoved(pointer)]),
                    baseline,
                    "hover moved tabs (light={light_theme}, tagged={tagged})"
                );
            }
            for _ in 0..3 {
                assert_eq!(render(vec![Event::PointerGone]), baseline);
            }
        }
    }
}

#[test]
fn a_single_navigation_click_selects_without_opening_a_connection() {
    let ctx = egui::Context::default();
    let profile = RemoteProfile {
        name: "selection-test".into(),
        host: "example.org".into(),
        user: "root".into(),
        ..Default::default()
    };
    let mut app = App::from_settings(
        &ctx,
        Settings {
            profiles: vec![profile],
            ..Default::default()
        },
        None,
        None,
    );
    let mut render = |events| {
        ctx.run(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1280.0, 800.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| app.sidebar(ctx, &mut None),
        )
    };
    for _ in 0..3 {
        render(vec![]);
    }
    let output = render(vec![]);
    let pos = output
        .shapes
        .iter()
        .find_map(|shape| {
            if let egui::epaint::Shape::Text(text) = &shape.shape
                && text.galley.job.text == "selection-test"
            {
                Some(text.pos + text.galley.rect.center().to_vec2())
            } else {
                None
            }
        })
        .expect("saved connection row");
    render(vec![Event::PointerMoved(pos)]);
    render(vec![press(pos, true)]);
    render(vec![press(pos, false)]);
    render(vec![Event::PointerGone]);
    assert_eq!(
        app.navigation_selection,
        Some(egui::Id::new(("navigation-ssh", 0usize)))
    );
    assert_eq!(app.tabs.len(), 1);
    assert!(app.login.is_none());
}

#[test]
fn recent_connections_select_on_single_click_and_connect_on_double_click() {
    fn render(
        app: &mut App,
        ctx: &egui::Context,
        events: Vec<Event>,
        time: f64,
    ) -> (egui::FullOutput, Option<Action>) {
        let mut action = None;
        let output = ctx.run(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1280.0, 800.0),
                )),
                events,
                time: Some(time),
                ..Default::default()
            },
            |ctx| app.sidebar(ctx, &mut action),
        );
        (output, action)
    }
    let ctx = egui::Context::default();
    let recent_key = "ssh:root@recent.invalid:22".to_string();
    let mut app = App::from_settings(
        &ctx,
        Settings {
            profiles: vec![RemoteProfile {
                name: "recent-double-click".into(),
                host: "recent.invalid".into(),
                user: "root".into(),
                ..Default::default()
            }],
            recent_connections: vec![recent_key.clone()],
            ..Default::default()
        },
        None,
        None,
    );
    for i in 0..3 {
        render(&mut app, &ctx, vec![], i as f64 * 0.1);
    }
    let (output, _) = render(&mut app, &ctx, vec![], 0.3);
    let pos = output
        .shapes
        .iter()
        .find_map(|shape| {
            if let egui::Shape::Text(text) = &shape.shape
                && text.galley.text() == "recent-double-click"
            {
                Some(text.pos + text.galley.rect.center().to_vec2())
            } else {
                None
            }
        })
        .expect("recent connection row");
    render(
        &mut app,
        &ctx,
        vec![Event::PointerMoved(pos), press(pos, true)],
        0.4,
    );
    let (_, action) = render(&mut app, &ctx, vec![press(pos, false)], 0.45);
    assert!(
        action.is_none(),
        "a single click must only select the recent row"
    );
    assert_eq!(
        app.navigation_selection,
        Some(egui::Id::new(("navigation-recent", &recent_key)))
    );
    assert!(app.login.is_none());
    render(&mut app, &ctx, vec![press(pos, true)], 0.5);
    let (_, action) = render(&mut app, &ctx, vec![press(pos, false)], 0.55);
    assert!(
        matches!(action, Some(Action::New(SessionKind::Ssh(profile)))
        if profile.host == "recent.invalid" && profile.user == "root")
    );
}

#[test]
fn a_background_reconnect_indicator_reconnects_and_activates_its_tab() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    let kind = app.tabs[0].panes[0].session.kind.clone();
    app.tabs[0].panes[0].session = Session::disconnected(kind.clone(), app.settings.scrollback);
    let previous = app.tabs[0].panes[0].session.terminal.clone();
    app.execute(Action::New(kind), &ctx);
    assert_eq!(app.active, 1);
    let other_session = app.tabs[1].panes[0].session.terminal.clone();
    let mut render = |events| {
        let mut action = None;
        let output = ctx.run(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1280.0, 800.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| app.topbar(ctx, &mut action),
        );
        (output, action)
    };
    for _ in 0..3 {
        render(vec![]);
    }
    let (output, _) = render(vec![]);
    let pos = output
        .shapes
        .iter()
        .find_map(|shape| {
            if let egui::epaint::Shape::Circle(circle) = &shape.shape
                && circle.radius == 4.5
                && circle.fill == Palette::new(false).muted
            {
                Some(circle.center)
            } else {
                None
            }
        })
        .expect("disconnected status dot");
    assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
        egui::epaint::Shape::Text(text) if text.galley.job.text == "未连接")));
    let (hover, _) = render(vec![Event::PointerMoved(pos)]);
    assert!(!hover.shapes.iter().any(|shape| matches!(&shape.shape,
        egui::epaint::Shape::Circle(circle) if circle.center == pos && circle.radius == 4.5)));
    render(vec![press(pos, true)]);
    let (_, action) = render(vec![press(pos, false)]);
    assert!(matches!(action, Some(Action::ReconnectTab(0))));
    app.execute(action.unwrap(), &ctx);
    assert_eq!(app.active, 0);
    assert_ne!(app.tabs[0].panes[0].session.link(), SessionStatus::Detached);
    assert!(Arc::ptr_eq(
        &previous,
        &app.tabs[0].panes[0].session.terminal
    ));
    assert!(Arc::ptr_eq(
        &other_session,
        &app.tabs[1].panes[0].session.terminal
    ));
    assert_eq!(app.tabs[1].panes[0].session.link(), SessionStatus::Live);
}

#[test]
fn navigation_selection_is_one_clicked_row_and_survives_tab_changes() {
    fn render(
        app: &mut App,
        ctx: &egui::Context,
        events: Vec<Event>,
        time: f64,
    ) -> egui::FullOutput {
        ctx.run(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1280.0, 800.0),
                )),
                events,
                time: Some(time),
                ..Default::default()
            },
            |ctx| app.sidebar(ctx, &mut None),
        )
    }
    fn selected_rows(output: &egui::FullOutput) -> Vec<Rect> {
        output
            .shapes
            .iter()
            .filter_map(|shape| {
                if let egui::epaint::Shape::Rect(rect) = &shape.shape
                    && rect.fill == Palette::new(false).accent
                    && rect.rect.width() == 3.0
                    && (20.0..=28.0).contains(&rect.rect.height())
                {
                    Some(rect.rect)
                } else {
                    None
                }
            })
            .collect()
    }
    let ctx = egui::Context::default();
    let mut app = App::from_settings(
        &ctx,
        Settings {
            profiles: vec![
                RemoteProfile {
                    name: "other-copy".into(),
                    user: "root".into(),
                    host: "same.example".into(),
                    ..Default::default()
                },
                RemoteProfile {
                    name: "selected-copy".into(),
                    user: "root".into(),
                    host: "same.example".into(),
                    ..Default::default()
                },
            ],
            recent_connections: vec!["ssh:root@same.example:22".into()],
            ..Default::default()
        },
        None,
        None,
    );
    for i in 0..3 {
        render(&mut app, &ctx, vec![], i as f64 * 0.1);
    }
    let output = render(&mut app, &ctx, vec![], 0.3);
    assert!(
        selected_rows(&output).is_empty(),
        "active shell must not select a row"
    );
    let pos = output
        .shapes
        .iter()
        .find_map(|shape| {
            if let egui::epaint::Shape::Text(text) = &shape.shape
                && text.galley.job.text == "selected-copy"
            {
                Some(text.pos + text.galley.rect.center().to_vec2())
            } else {
                None
            }
        })
        .unwrap();
    render(
        &mut app,
        &ctx,
        vec![Event::PointerMoved(pos), press(pos, true)],
        0.4,
    );
    render(&mut app, &ctx, vec![press(pos, false)], 0.5);
    render(&mut app, &ctx, vec![Event::PointerGone], 0.6);
    let output = render(&mut app, &ctx, vec![], 0.9);
    assert_eq!(
        app.navigation_selection,
        Some(egui::Id::new(("navigation-ssh", 1usize)))
    );
    let selected = selected_rows(&output);
    assert_eq!(
        selected.len(),
        1,
        "same-address rows must not share selection"
    );
    assert!(selected[0].y_range().contains(pos.y));
    app.execute(
        Action::New(SessionKind::Local(app.settings.default_shell.clone())),
        &ctx,
    );
    let output = render(&mut app, &ctx, vec![], 1.0);
    assert_eq!(
        selected_rows(&output),
        selected,
        "opening a tab changed mouse selection"
    );
    app.execute(Action::ActivateTab(0), &ctx);
    let output = render(&mut app, &ctx, vec![], 1.1);
    assert_eq!(
        selected_rows(&output),
        selected,
        "activating a tab changed mouse selection"
    );
}

#[test]
fn the_status_button_enters_and_leaves_fullscreen() {
    for fullscreen in [false, true] {
        let ctx = egui::Context::default();
        let mut app = App::from_settings(&ctx, Settings::default(), None, None);
        let size = Vec2::new(1280.0, 800.0);
        let mut render = |events| {
            let mut raw = RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            };
            raw.viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .fullscreen = Some(fullscreen);
            ctx.run(raw, |ctx| app.status_bar(ctx, &mut None, app.palette))
        };
        for _ in 0..3 {
            render(vec![]);
        }
        // The rightmost status control has a 20pt hit region inside the 12pt margin.
        let pos = egui::pos2(size.x - 22.0, size.y - 14.0);
        render(vec![Event::PointerMoved(pos)]);
        render(vec![press(pos, true)]);
        let output = render(vec![press(pos, false)]);
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::Fullscreen(!fullscreen))
        );
    }
}

#[test]
fn navigation_search_keeps_focus_and_arrows_choose_a_saved_connection() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(
        &ctx,
        Settings {
            profiles: vec![
                RemoteProfile {
                    name: "prod-a".into(),
                    host: "a.invalid".into(),
                    ..Default::default()
                },
                RemoteProfile {
                    name: "prod-b".into(),
                    host: "b.invalid".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        None,
        None,
    );
    let size = Vec2::new(1280.0, 800.0);
    for _ in 0..3 {
        frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    }
    let id = sidebar::connection_search_id();
    let pos = ctx.read_response(id).expect("search field").rect.center();
    frame(
        &mut app,
        &ctx,
        vec![Event::PointerMoved(pos), press(pos, true)],
        Modifiers::NONE,
        size,
    );
    frame(
        &mut app,
        &ctx,
        vec![press(pos, false)],
        Modifiers::NONE,
        size,
    );
    frame(
        &mut app,
        &ctx,
        vec![Event::Text("prod".into())],
        Modifiers::NONE,
        size,
    );
    assert_eq!(app.connection_filter, "prod");
    assert!(
        ctx.memory(|memory| memory.has_focus(id)),
        "terminal stole search focus"
    );
    let key = |key| Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    frame(
        &mut app,
        &ctx,
        vec![key(Key::ArrowDown)],
        Modifiers::NONE,
        size,
    );
    assert_eq!(app.connection_candidate, Some(0));
    frame(
        &mut app,
        &ctx,
        vec![key(Key::ArrowDown)],
        Modifiers::NONE,
        size,
    );
    assert_eq!(app.connection_candidate, Some(1));
    frame(&mut app, &ctx, vec![key(Key::Enter)], Modifiers::NONE, size);
    assert_eq!(app.login.as_ref().unwrap().profile.host, "b.invalid");
    assert!(app.connection_filter.is_empty());
}

#[test]
fn quick_credentials_never_enter_recent_or_saved_profiles() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    app.connection_filter = "alice:private-secret@example.org:2222".into();
    let target = quick_connect::parse(&app.connection_filter).unwrap();
    app.execute(Action::QuickConnect(target), &ctx);
    assert!(app.connection_filter.is_empty());
    assert_eq!(
        app.settings.recent_connections,
        vec!["ssh:alice@example.org:2222"]
    );
    let kind = app.recent_profiles[0].clone();
    app.execute(Action::SaveConnection(kind.clone()), &ctx);
    app.execute(Action::SaveConnection(kind), &ctx);
    assert_eq!(
        app.settings.profiles.len(),
        1,
        "saving twice duplicated a connection"
    );
    app.snapshot();
    assert!(
        !serde_json::to_string(&app.settings)
            .unwrap()
            .contains("private-secret")
    );
}

#[test]
fn a_bare_ip_opens_credentials_without_starting_a_connection() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    app.execute(
        Action::QuickConnect(quick_connect::parse("192.0.2.1").unwrap()),
        &ctx,
    );
    let mut login = app.login.take().unwrap();
    let mut open = true;
    for _ in 0..3 {
        let _ = ctx.run(Default::default(), |ctx| {
            login.show(ctx, &mut open, app.palette);
        });
    }
    let output = ctx.run(Default::default(), |ctx| {
        login.show(ctx, &mut open, app.palette);
    });
    assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
        egui::epaint::Shape::Text(text) if text.galley.job.text == "密码")));
    assert_eq!(app.settings.recent_connections, vec!["ssh:@192.0.2.1:22"]);
}

#[test]
fn activating_an_inactive_ssh_tab_does_not_reconnect_it() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    let remote = SessionKind::Ssh(RemoteProfile {
        host: "example.org".into(),
        ..Default::default()
    });
    app.tabs[0].panes[0].session = Session::disconnected(remote, app.settings.scrollback);
    app.execute(
        Action::New(SessionKind::Local(app.settings.default_shell.clone())),
        &ctx,
    );
    app.execute(Action::ActivateTab(0), &ctx);
    assert_eq!(app.active, 0);
    assert!(app.login.is_none());
    assert_eq!(app.tabs[0].panes[0].session.link(), SessionStatus::Detached);
}

#[test]
fn menu_exit_uses_the_existing_live_session_confirmation() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(&ctx, Settings::default(), None, None);
    app.execute(Action::Exit, &ctx);
    assert!(app.confirm_exit);
    assert!(!app.exit_confirmed);
    app.settings.confirm_on_exit = false;
    let output = ctx.run(RawInput::default(), |ctx| app.execute(Action::Exit, ctx));
    assert!(
        output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .contains(&egui::ViewportCommand::Close)
    );
}

/// Opt-in timing probe. Run in a release test binary with --ignored --nocapture.
/// It reports frame cost under repeatable output, history and multi-tab loads.
#[test]
#[ignore]
fn benchmark_ui_frames() {
    let ctx = egui::Context::default();
    let settings = Settings {
        scrollback: 50_000,
        ..Settings::default()
    };
    let mut app = App::from_settings(&ctx, settings, None, None);
    let size = Vec2::new(1280.0, 800.0);
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
    let mut samples = Vec::new();
    for scenario in ["idle", "output", "history", "four-tabs", "input"] {
        if scenario == "four-tabs" {
            for _ in 0..3 {
                app.execute(
                    Action::New(SessionKind::Local(app.settings.default_shell.clone())),
                    &ctx,
                );
            }
        }
        for i in 0..40 {
            let pane = &app.tabs[app.active].panes[0];
            if scenario == "output" || scenario == "history" {
                let mut terminal = pane.session.terminal.lock().unwrap();
                terminal
                    .process(format!("{i:04} sample output {}\r\n", "x".repeat(120)).as_bytes());
                if scenario == "history" {
                    terminal.scroll(3);
                }
            }
            let events = if scenario == "input" {
                vec![Event::Text("x".into())]
            } else {
                Vec::new()
            };
            let start = std::time::Instant::now();
            frame(&mut app, &ctx, events, Modifiers::NONE, size);
            if i >= 5 {
                samples.push(start.elapsed().as_secs_f64() * 1000.0);
            }
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "UI {scenario}: p50={:.2}ms p95={:.2}ms",
            samples[samples.len() / 2],
            samples[samples.len() * 95 / 100]
        );
        samples.clear();
    }
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
    // The chooser owns the keyboard until it closes; keep the new session
    // before exercising terminal input.
    assert!(app.split_chooser.is_some());
    app.split_chooser = None;
    frame(&mut app, &ctx, vec![], Modifiers::NONE, size);
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
