//! Small drawing helpers the panels share: tag colours, the colour picker and
//! the accelerator text.

use super::*;

/// Parses `#rrggbb` into a colour. Empty or malformed input means no colour.
pub(super) fn parse_tag_color(hex: &str) -> Option<egui::Color32> {
    let hex = hex.trim().strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some(egui::Color32::from_rgb(
        (value >> 16) as u8,
        (value >> 8) as u8,
        value as u8,
    ))
}

/// Mixes `color` into `base`; `t` is how much of `color` shows.
pub(super) fn blend(base: egui::Color32, color: egui::Color32, t: f32) -> egui::Color32 {
    let mix = |a: u8, b: u8| {
        (a as f32 * (1.0 - t) + b as f32 * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    egui::Color32::from_rgb(
        mix(base.r(), color.r()),
        mix(base.g(), color.g()),
        mix(base.b(), color.b()),
    )
}

/// The tag colour for a session: the connection's own colour if it has one,
/// otherwise its group's. Local sessions have neither.
pub(super) fn connection_color(
    kind: &SessionKind,
    group_colors: &std::collections::BTreeMap<String, String>,
) -> Option<egui::Color32> {
    let (color, group) = match kind {
        SessionKind::Ssh(profile) | SessionKind::Sftp(profile) => (&profile.color, &profile.group),
        SessionKind::Serial(profile) => (&profile.color, &profile.group),
        SessionKind::Local(_) => return None,
    };
    parse_tag_color(color).or_else(|| group_colors.get(group).and_then(|hex| parse_tag_color(hex)))
}

/// A row of colour dots plus a "no colour" dot. Returns true when the selection
/// changed; `selected` is the stored `#rrggbb`, empty for none.
pub(super) fn color_picker(ui: &mut egui::Ui, selected: &mut String, p: Palette) -> bool {
    let mut changed = false;
    for hex in std::iter::once("").chain(TAG_COLORS.iter().copied()) {
        let (rect, response) =
            ui.allocate_exact_size(egui::Vec2::splat(16.0), egui::Sense::click());
        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            let centre = rect.center();
            match parse_tag_color(hex) {
                Some(color) => {
                    painter.circle_filled(centre, 6.0, color);
                }
                None => {
                    let stroke = egui::Stroke::new(1.0_f32, p.muted);
                    painter.circle_stroke(centre, 6.0, stroke);
                    painter.line_segment(
                        [
                            centre + egui::vec2(-4.0, 4.0),
                            centre + egui::vec2(4.0, -4.0),
                        ],
                        stroke,
                    );
                }
            }
            if *selected == hex {
                painter.circle_stroke(centre, 8.0, egui::Stroke::new(1.5_f32, p.text));
            }
        }
        if response
            .on_hover_text(if hex.is_empty() { "无颜色" } else { hex })
            .clicked()
        {
            *selected = hex.to_string();
            changed = true;
        }
    }
    changed
}

/// macOS calls the key Option; the modifier itself is the same. The word is
/// used rather than the ⌥ symbol because the loaded UI fonts do not carry it —
/// it would render as a tofu box, like every glyph the app draws instead.
pub(super) fn alt_accel(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("Option+{key}")
    } else {
        format!("Alt+{key}")
    }
}

pub(super) fn link_color(link: SessionStatus, p: Palette) -> egui::Color32 {
    match link {
        SessionStatus::Live => p.ok,
        SessionStatus::Detached => p.muted,
        SessionStatus::Lost => p.danger,
    }
}
