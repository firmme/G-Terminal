use super::*;

#[test]
fn copy_names_are_numbered_when_taken() {
    let taken = vec!["prod 副本".to_string(), "prod 副本 2".to_string()];
    assert_eq!(unique_copy_name("prod", &taken), "prod 副本 3");
    assert_eq!(unique_copy_name("dev", &[]), "dev 副本");
}

#[test]
fn the_color_picker_lays_out_and_starts_unselected() {
    let ctx = egui::Context::default();
    let mut selected = String::new();
    let mut changed = false;
    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            changed = color_picker(ui, &mut selected, Palette::new(false));
        });
    });
    assert!(!changed, "nothing is picked without a click");
    assert!(selected.is_empty());
}

#[test]
fn tag_colors_parse_and_prefer_the_connection() {
    use g_terminal::config::{RemoteProfile, TAG_COLORS};
    use std::collections::BTreeMap;

    assert_eq!(
        parse_tag_color("#8bd5ca"),
        Some(egui::Color32::from_rgb(0x8b, 0xd5, 0xca))
    );
    assert_eq!(parse_tag_color(""), None);
    assert_eq!(parse_tag_color("#xyzxyz"), None);

    let mut groups = BTreeMap::new();
    groups.insert("prod".to_string(), TAG_COLORS[0].to_string());

    // The group colour applies while the connection has none.
    let grouped = RemoteProfile {
        group: "prod".into(),
        ..Default::default()
    };
    assert_eq!(
        connection_color(&SessionKind::Ssh(grouped), &groups),
        parse_tag_color(TAG_COLORS[0])
    );

    // The connection's own colour wins over the group's.
    let own = RemoteProfile {
        group: "prod".into(),
        color: TAG_COLORS[2].into(),
        ..Default::default()
    };
    assert_eq!(
        connection_color(&SessionKind::Ssh(own), &groups),
        parse_tag_color(TAG_COLORS[2])
    );

    // Local sessions carry no colour at all.
    assert_eq!(
        connection_color(&SessionKind::Local("shell".into()), &groups),
        None
    );
}
