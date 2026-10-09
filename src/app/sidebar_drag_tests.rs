use super::*;
use egui::{Event, Modifiers, Pos2, RawInput, Vec2};

struct Harness {
    ctx: egui::Context,
    app: App,
    time: f64,
}

impl Harness {
    fn new() -> Self {
        let ctx = egui::Context::default();
        let app = App::from_settings(
            &ctx,
            Settings {
                groups: vec!["工作".into(), "新分组 1".into(), "新分组 3".into()],
                profiles: vec![
                    RemoteProfile {
                        name: "saved-a".into(),
                        host: "192.0.2.1".into(),
                        ..Default::default()
                    },
                    RemoteProfile {
                        name: "saved-b".into(),
                        host: "192.0.2.1".into(),
                        group: "工作".into(),
                        ..Default::default()
                    },
                ],
                serial_profiles: vec![SerialProfile {
                    name: "serial-a".into(),
                    port: "COM199".into(),
                    ..Default::default()
                }],
                ssh_config_profiles: vec![RemoteProfile {
                    name: "imported-a".into(),
                    host: "192.0.2.2".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            None,
            None,
        );
        let mut harness = Self {
            ctx,
            app,
            time: 0.0,
        };
        for _ in 0..4 {
            harness.frame(vec![]);
        }
        harness
    }

    fn frame(&mut self, events: Vec<Event>) -> (egui::FullOutput, Option<Action>) {
        self.time += 0.1;
        let mut action = None;
        let output = self.ctx.run(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0))),
                events,
                time: Some(self.time),
                ..Default::default()
            },
            |ctx| {
                self.app.sidebar(ctx, &mut action);
                self.app.panes(ctx, &mut action, self.app.palette);
            },
        );
        (output, action)
    }

    fn position(&mut self, label: &str) -> Pos2 {
        let (output, _) = self.frame(vec![]);
        text_position(&output, label).unwrap_or_else(|| panic!("missing row: {label}"))
    }

    fn start_drag(&mut self, label: &str) {
        let pos = self.position(label);
        self.frame(vec![Event::PointerMoved(pos), pointer(pos, true)]);
        self.frame(vec![Event::PointerMoved(pos + Vec2::new(20.0, 0.0))]);
        assert!(
            egui::DragAndDrop::has_payload_of_type::<sidebar::ConnectionDrag>(&self.ctx),
            "{label} is not draggable"
        );
    }

    fn release(&mut self, pos: Pos2) -> Option<Action> {
        self.frame(vec![Event::PointerMoved(pos)]);
        let (_, action) = self.frame(vec![pointer(pos, false)]);
        assert!(!egui::DragAndDrop::has_any_payload(&self.ctx));
        action
    }
}

fn pointer(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        pressed,
        button: egui::PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }
}

fn text_position(output: &egui::FullOutput, label: &str) -> Option<Pos2> {
    output.shapes.iter().find_map(|shape| {
        if let egui::Shape::Text(text) = &shape.shape
            && text.galley.text() == label
        {
            Some(text.pos + text.galley.rect.center().to_vec2())
        } else {
            None
        }
    })
}

fn highlighted_group(output: &egui::FullOutput, pos: Pos2, p: Palette) -> bool {
    output.shapes.iter().any(|shape| {
        matches!(&shape.shape, egui::Shape::Rect(rect)
        if rect.rect.contains(pos) && rect.stroke.color == p.accent && rect.stroke.width == 1.0)
    })
}

#[test]
fn saved_ssh_and_serial_move_to_groups_without_touching_same_address_rows() {
    for source in ["saved-a", "serial-a"] {
        let mut h = Harness::new();
        let group = h.position("工作 (1)");
        h.start_drag(source);
        let (hover, action) = h.frame(vec![Event::PointerMoved(group)]);
        assert!(action.is_none());
        assert!(highlighted_group(&hover, group, h.app.palette));
        assert!(h.app.settings.profiles[0].group.is_empty());
        assert!(h.app.settings.serial_profiles[0].group.is_empty());
        let action = h.release(group).expect("group drop");
        assert!(matches!(action, Action::MoveConnection(_, Some(_))));
        h.app.execute(action, &h.ctx);
        assert_eq!(h.app.settings.profiles[1].group, "工作");
        if source == "saved-a" {
            assert_eq!(h.app.settings.profiles[0].group, "工作");
            assert!(h.app.settings.serial_profiles[0].group.is_empty());
        } else {
            assert_eq!(h.app.settings.serial_profiles[0].group, "工作");
            assert!(h.app.settings.profiles[0].group.is_empty());
        }
        assert_eq!(
            h.app.tabs.len(),
            1,
            "moving a profile must not open a session"
        );
    }
}

#[test]
fn dropping_at_the_end_creates_the_next_group_and_cancelling_creates_nothing() {
    let mut h = Harness::new();
    let (normal, _) = h.frame(vec![]);
    assert!(text_position(&normal, "新建分组 · 新分组 4").is_none());
    h.start_drag("saved-a");
    let target = h.position("新建分组 · 新分组 4");
    assert_eq!(h.app.settings.groups.len(), 3);
    let action = h.release(target).expect("new group drop");
    h.app.execute(action, &h.ctx);
    assert_eq!(h.app.settings.profiles[0].group, "新分组 4");
    assert_eq!(h.app.settings.groups.last().unwrap(), "新分组 4");
    h.start_drag("saved-a");
    h.position("新建分组 · 新分组 5");
    h.frame(vec![Event::Key {
        key: Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    assert!(!egui::DragAndDrop::has_any_payload(&h.ctx));
    assert_eq!(h.app.settings.groups.len(), 4);
    assert_eq!(h.app.settings.profiles[0].group, "新分组 4");
}

#[test]
fn an_empty_default_group_remains_a_drop_target() {
    let mut h = Harness::new();
    h.app.settings.profiles[0].group = "工作".into();
    h.app.settings.serial_profiles[0].group = "工作".into();
    h.start_drag("saved-a");
    let target = h.position("默认分组 (0)");
    let action = h.release(target).expect("empty default group drop");
    h.app.execute(action, &h.ctx);
    assert!(h.app.settings.profiles[0].group.is_empty());
    assert_eq!(h.app.settings.profiles[1].group, "工作");
}

#[test]
fn imported_hosts_and_local_shells_drag_to_the_console_but_ignore_groups() {
    let shell = local_shells()[0].label.clone();
    for source in ["imported-a", &shell] {
        let mut h = Harness::new();
        let group = h.position("工作 (1)");
        h.start_drag(source);
        let (hover, _) = h.frame(vec![Event::PointerMoved(group)]);
        assert!(!highlighted_group(&hover, group, h.app.palette));
        assert!(text_position(&hover, "新建分组 · 新分组 4").is_none());
        assert!(
            h.release(group).is_none(),
            "read-only rows must not change groups"
        );
        assert!(h.app.settings.ssh_config_profiles[0].group.is_empty());
        h.start_drag(source);
        let action = h.release(Pos2::new(700.0, 350.0)).expect("console drop");
        assert!(matches!(action, Action::New(_)));
        h.app.execute(action, &h.ctx);
        if source == "imported-a" {
            assert_eq!(h.app.login.as_ref().unwrap().profile.host, "192.0.2.2");
        } else {
            assert_eq!(h.app.tabs.len(), 2);
            assert_eq!(h.app.active, 1);
        }
        assert_eq!(h.app.settings.groups.len(), 3);
    }
}

#[test]
fn dragging_to_the_bottom_scrolls_to_the_new_group_target() {
    let mut h = Harness::new();
    for index in 0..50 {
        h.app.settings.profiles.push(RemoteProfile {
            name: format!("extra-{index}"),
            host: "192.0.2.3".into(),
            ..Default::default()
        });
    }
    for _ in 0..3 {
        h.frame(vec![]);
    }
    h.start_drag("saved-a");
    let mut target = None;
    for _ in 0..250 {
        let (output, _) = h.frame(vec![Event::PointerMoved(Pos2::new(100.0, 785.0))]);
        if let Some(pos) = text_position(&output, "新建分组 · 新分组 4")
            && output.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Text(text) if text.galley.text() == "新建分组 · 新分组 4" && shape.clip_rect.contains(pos))) {
            target = Some(pos); break;
        }
    }
    let action = h
        .release(target.expect("new group target should become reachable"))
        .expect("new group drop after scrolling");
    h.app.execute(action, &h.ctx);
    assert_eq!(h.app.settings.profiles[0].group, "新分组 4");
}

#[test]
fn saved_connection_console_drop_opens_a_session_and_keeps_its_group() {
    let mut h = Harness::new();
    h.start_drag("saved-b");
    let action = h
        .release(Pos2::new(700.0, 350.0))
        .expect("saved connection console drop");
    h.app.execute(action, &h.ctx);
    assert_eq!(h.app.login.as_ref().unwrap().profile.host, "192.0.2.1");
    assert_eq!(h.app.settings.profiles[1].group, "工作");
}
