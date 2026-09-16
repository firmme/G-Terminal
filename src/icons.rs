//! Hand-drawn UI glyphs.
//!
//! Every icon here is painted from primitives rather than taken from a font. The
//! font chain this app loads (Ubuntu-Light → NotoEmoji → emoji-icon-font → 微软雅黑)
//! has no dependable coverage for symbol codepoints, which is why the first
//! titlebar buttons rendered as tofu boxes. Painting also keeps the icons crisp
//! at any DPI, and costs no binary asset.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::theme::Palette;

/// Every glyph the UI draws.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Terminal,
    Host,
    FolderUp,
    Home,
    Refresh,
    Group,
    Settings,
    Help,
    SplitHorizontal,
    SplitVertical,
    ClosePane,
    Restart,
    Search,
    Plus,
    ChevronLeft,
    ChevronRight,
    NewFolder,
    File,
    Close,
    Maximize,
    Restore,
    Minimize,
    /// A directory in a file listing.
    Folder,
    /// File-type glyphs. They share one page outline and differ in the mark
    /// inside it, so the set reads as a family rather than as unrelated drawings.
    FileText,
    FileCode,
    FileImage,
    FileArchive,
    FileMedia,
    FileBinary,
}

/// The three sizes the UI asks for. The stroke thins as the glyph shrinks, so a
/// small icon does not turn into a blob.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Size {
    /// Inline with a text row.
    Row,
    /// A standalone button.
    Button,
    /// In the window chrome, where buttons are widest.
    Title,
}

impl Size {
    fn extent(self) -> f32 {
        match self {
            Size::Row => 15.0,
            Size::Button => 16.0,
            Size::Title => 13.0,
        }
    }
    pub fn stroke(self) -> f32 {
        match self {
            Size::Row => 1.2,
            Size::Button => 1.3,
            Size::Title => 1.25,
        }
    }
    /// The box a button of this size occupies. Public so a layout that places a
    /// widget after an icon can reserve the icon's room.
    pub fn button(self) -> Vec2 {
        match self {
            Size::Row => Vec2::new(20.0, 18.0),
            Size::Button => Vec2::new(24.0, 20.0),
            Size::Title => Vec2::new(32.0, 20.0),
        }
    }
}

/// Paints `icon` centred in `rect`. Geometry is expressed as fractions of the
/// box, so one drawing serves every size.
pub fn draw(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32, width: f32) {
    let stroke = Stroke::new(width, color);
    let c = rect.center();
    // Half the shorter side, so a wide button box still yields a square glyph.
    let r = rect.width().min(rect.height()) * 0.5;
    let p = |x: f32, y: f32| c + Vec2::new(x * r, y * r);
    let seg = |a: (f32, f32), b: (f32, f32)| {
        painter.line_segment([p(a.0, a.1), p(b.0, b.1)], stroke);
    };
    let boxed = |x0: f32, y0: f32, x1: f32, y1: f32| {
        painter.rect_stroke(
            Rect::from_min_max(p(x0, y0), p(x1, y1)),
            0,
            stroke,
            StrokeKind::Inside,
        );
    };
    // A ring, for the round glyphs. Drawn as segments because the painter has no
    // arc primitive; eight is enough at these sizes.
    let ring = |radius: f32, from: f32, to: f32| {
        let steps = 8;
        let mut previous = None;
        for step in 0..=steps {
            let t = from + (to - from) * step as f32 / steps as f32;
            let point = p(radius * t.cos(), radius * t.sin());
            if let Some(previous) = previous {
                painter.line_segment([previous, point], stroke);
            }
            previous = Some(point);
        }
    };
    let arrow = |tip: (f32, f32), dx: f32, dy: f32| {
        seg(tip, (tip.0 - dx, tip.1 - dy));
        seg(tip, (tip.0 + dx, tip.1 - dy));
    };

    match icon {
        Icon::Terminal => {
            // A window with a prompt inside it: what the app opens.
            boxed(-1.0, -0.85, 1.0, 0.85);
            seg((-0.55, -0.25), (-0.1, 0.05));
            seg((-0.1, 0.05), (-0.55, 0.35));
            seg((0.1, 0.4), (0.6, 0.4));
        }
        Icon::Host => {
            // A monitor on a stand: a remote machine.
            boxed(-1.0, -0.8, 1.0, 0.35);
            seg((0.0, 0.35), (0.0, 0.7));
            seg((-0.45, 0.8), (0.45, 0.8));
        }
        Icon::FolderUp | Icon::NewFolder => {
            // Body, with the tab that makes it read as a folder.
            seg((-1.0, 0.7), (-1.0, -0.7));
            seg((-1.0, -0.7), (-0.35, -0.7));
            seg((-0.35, -0.7), (-0.05, -0.35));
            seg((-0.05, -0.35), (1.0, -0.35));
            seg((1.0, -0.35), (1.0, 0.7));
            seg((-1.0, 0.7), (1.0, 0.7));
            if icon == Icon::FolderUp {
                seg((0.0, 0.45), (0.0, -0.05));
                arrow((0.0, -0.15), 0.22, 0.22);
            } else if icon == Icon::NewFolder {
                seg((0.0, 0.45), (0.0, 0.0));
                seg((-0.22, 0.22), (0.22, 0.22));
            }
        }
        Icon::Home => {
            let roof = (-0.15, -1.0);
            seg((-1.0, roof.1 + 0.55), roof);
            seg(roof, (1.0, roof.1 + 0.55));
            seg((-0.7, roof.1 + 0.5), (-0.7, 0.85));
            seg((0.7, roof.1 + 0.5), (0.7, 0.85));
            seg((-0.7, 0.85), (0.7, 0.85));
        }
        Icon::Refresh => {
            // Three quarters of a circle with an arrowhead closing the gap.
            ring(0.8, -2.4, 1.9);
            let tip = p(0.8 * 1.9f32.cos(), 0.8 * 1.9f32.sin());
            painter.line_segment([tip, tip + Vec2::new(-0.34 * r, -0.24 * r)], stroke);
            painter.line_segment([tip, tip + Vec2::new(0.06 * r, -0.42 * r)], stroke);
        }
        Icon::Group => {
            // Two overlapping panes.
            boxed(-1.0, -0.75, 0.45, 0.75);
            boxed(-0.45, -0.35, 1.0, 1.0);
        }
        Icon::Settings => {
            // A hub with spokes.
            ring(0.45, 0.0, std::f32::consts::TAU);
            for step in 0..6 {
                let t = step as f32 * std::f32::consts::TAU / 6.0;
                seg(
                    (0.5 * t.cos(), 0.5 * t.sin()),
                    (0.95 * t.cos(), 0.95 * t.sin()),
                );
            }
        }
        Icon::Help => {
            ring(0.9, 0.0, std::f32::consts::TAU);
            seg((0.0, -0.25), (0.0, 0.15));
            painter.circle_filled(p(0.0, 0.45), (width * 0.9).max(1.0), color);
        }
        Icon::SplitHorizontal => {
            boxed(-1.0, -0.8, 1.0, 0.8);
            seg((0.0, -0.8), (0.0, 0.8));
        }
        Icon::SplitVertical => {
            boxed(-1.0, -0.8, 1.0, 0.8);
            seg((-1.0, 0.0), (1.0, 0.0));
        }
        Icon::ClosePane => {
            // A pane with its corner struck out.
            boxed(-1.0, -0.8, 1.0, 0.8);
            seg((-0.45, -0.35), (0.05, 0.35));
            seg((0.05, -0.35), (-0.45, 0.35));
        }
        Icon::Restart => {
            // A power symbol: an open ring with a stem through the gap.
            ring(0.85, -1.9, 1.9);
            seg((0.0, -1.05), (0.0, -0.2));
        }
        Icon::Search => {
            ring(0.6, 0.0, std::f32::consts::TAU);
            seg((0.45, 0.45), (0.95, 0.95));
        }
        Icon::Plus => {
            seg((-0.7, 0.0), (0.7, 0.0));
            seg((0.0, -0.7), (0.0, 0.7));
        }
        Icon::ChevronLeft => {
            seg((0.35, -0.7), (-0.35, 0.0));
            seg((-0.35, 0.0), (0.35, 0.7));
        }
        Icon::ChevronRight => {
            seg((-0.35, -0.7), (0.35, 0.0));
            seg((0.35, 0.0), (-0.35, 0.7));
        }
        Icon::Folder => {
            // The same body the other folder glyphs use, without an inner mark.
            seg((-1.0, 0.7), (-1.0, -0.7));
            seg((-1.0, -0.7), (-0.35, -0.7));
            seg((-0.35, -0.7), (-0.05, -0.35));
            seg((-0.05, -0.35), (1.0, -0.35));
            seg((1.0, -0.35), (1.0, 0.7));
            seg((-1.0, 0.7), (1.0, 0.7));
        }
        Icon::File
        | Icon::FileText
        | Icon::FileCode
        | Icon::FileImage
        | Icon::FileArchive
        | Icon::FileMedia
        | Icon::FileBinary => {
            // A page with a folded corner. Every file glyph starts from this, so
            // the set reads as one family.
            seg((-0.7, -1.0), (0.35, -1.0));
            seg((0.35, -1.0), (0.8, -0.55));
            seg((0.8, -0.55), (0.8, 1.0));
            seg((0.8, 1.0), (-0.7, 1.0));
            seg((-0.7, 1.0), (-0.7, -1.0));
            seg((0.35, -1.0), (0.35, -0.55));
            seg((0.35, -0.55), (0.8, -0.55));
            // The mark says which kind; the colour says it again, so the two
            // signals agree rather than compete.
            match icon {
                Icon::File => {}
                Icon::FileText => {
                    // Lines of prose, full width.
                    for y in [0.15, 0.5, 0.85] {
                        seg((-0.45, y), (0.6, y));
                    }
                }
                Icon::FileCode => {
                    // Angle brackets.
                    seg((-0.1, 0.2), (-0.5, 0.5));
                    seg((-0.5, 0.5), (-0.1, 0.8));
                    seg((0.25, 0.2), (0.65, 0.5));
                    seg((0.65, 0.5), (0.25, 0.8));
                }
                Icon::FileImage => {
                    // A horizon with a sun above it.
                    seg((-0.45, 0.85), (-0.05, 0.35));
                    seg((-0.05, 0.35), (0.2, 0.65));
                    seg((0.2, 0.65), (0.4, 0.45));
                    seg((0.4, 0.45), (0.65, 0.85));
                    painter.circle_filled(p(0.35, 0.15), (width * 0.7).max(0.8), color);
                }
                Icon::FileArchive => {
                    // A zip's teeth: short dashes down the middle, deliberately
                    // narrower than the text lines so the two do not read alike.
                    for y in [0.1, 0.45, 0.8] {
                        seg((0.0, y), (0.3, y));
                    }
                    seg((0.15, 0.1), (0.15, 0.45));
                    seg((0.15, 0.45), (0.15, 0.8));
                }
                Icon::FileMedia => {
                    // A play triangle.
                    seg((-0.2, 0.2), (0.4, 0.5));
                    seg((0.4, 0.5), (-0.2, 0.8));
                    seg((-0.2, 0.8), (-0.2, 0.2));
                }
                Icon::FileBinary => {
                    // A filled block: opaque bytes, as opposed to readable text.
                    painter.rect_filled(
                        Rect::from_min_max(p(-0.25, 0.25), p(0.35, 0.85)),
                        0,
                        color,
                    );
                }
                _ => {}
            }
        }
        Icon::Close => {
            // Kept close to the size of the other window buttons; a full-width X
            // reads as heavier than its neighbours and looks out of place.
            let d = 0.5;
            seg((-d, -d), (d, d));
            seg((d, -d), (-d, d));
        }
        Icon::Minimize => seg((-0.45, 0.0), (0.45, 0.0)),
        Icon::Maximize => boxed(-0.45, -0.45, 0.45, 0.45),
        Icon::Restore => {
            // The back square shows only its top and left edges from behind the
            // front one, which is the usual shape for "restore down".
            seg((-0.5, -0.5), (0.05, -0.5));
            seg((-0.5, -0.5), (-0.5, 0.05));
            boxed(-0.05, -0.05, 0.5, 0.5);
        }
    }
}

/// A window-chrome button: wider than an inline one, and the close button turns
/// red under the cursor the way a native title bar does.
pub fn window_button(
    ui: &mut egui::Ui,
    icon: Icon,
    p: Palette,
    danger_hover: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Size::Title.button(), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let hovered = response.hovered();
    if hovered {
        let fill = if danger_hover {
            p.danger
        } else {
            ui.style().interact_selectable(&response, false).bg_fill
        };
        ui.painter().rect_filled(
            rect,
            ui.style().visuals.widgets.inactive.corner_radius,
            fill,
        );
    }
    let color = match (hovered, danger_hover) {
        (true, true) => Color32::WHITE,
        (true, false) => p.text,
        _ => p.muted,
    };
    draw(ui.painter(), rect, icon, color, Size::Title.stroke());
    response
}

/// A clickable icon with the usual hover treatment. Returns the response so
/// callers can chain `.on_hover_text(...)` and `.clicked()`.
pub fn icon_button(ui: &mut egui::Ui, icon: Icon, p: Palette, size: Size) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size.button(), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let hovered = response.hovered();
    if hovered {
        let visuals = ui.style().interact_selectable(&response, false);
        ui.painter().rect_filled(
            rect,
            ui.style().visuals.widgets.inactive.corner_radius,
            visuals.bg_fill,
        );
    }
    draw(
        ui.painter(),
        rect,
        icon,
        if hovered { p.text } else { p.muted },
        size.stroke(),
    );
    response
}

/// A one-character button that highlights under the cursor.
///
/// For glyphs the font actually carries — `×`, for instance, is ordinary Latin-1
/// punctuation, unlike the `✕` and `▢` that prompted hand-drawing everything. A
/// drawn stroke would be heavier than the text it sits beside.
pub fn glyph_button(ui: &mut egui::Ui, glyph: &str, p: Palette, tip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        if hovered {
            let visuals = ui.style().interact_selectable(&response, false);
            ui.painter().rect_filled(
                rect,
                ui.style().visuals.widgets.inactive.corner_radius,
                visuals.bg_fill,
            );
        }
        let font =
            egui::FontId::proportional(ui.style().text_styles[&egui::TextStyle::Button].size);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            glyph,
            font,
            if hovered { p.text } else { p.muted },
        );
    }
    response.on_hover_text(tip)
}

/// A full-width row with a leading icon and an optional shortcut on the right.
/// Used by the sidebar and the main menu, whose rows are otherwise plain text.
pub fn icon_row(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    shortcut: Option<&str>,
    p: Palette,
) -> egui::Response {
    const HEIGHT: f32 = 22.0;
    // In a menu popup `available_width` is generous and the row would stretch the
    // menu across the screen, so the width is bounded rather than taken as-is.
    let width = (ui.available_width() - 8.0).clamp(180.0, 280.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, HEIGHT), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        if hovered {
            let visuals = ui.style().interact_selectable(&response, false);
            ui.painter().rect_filled(
                rect,
                ui.style().visuals.widgets.inactive.corner_radius,
                visuals.bg_fill,
            );
        }
        let icon_rect = Rect::from_center_size(
            Pos2::new(rect.left() + HEIGHT * 0.5, rect.center().y),
            Vec2::splat(HEIGHT - 6.0),
        );
        draw(
            ui.painter(),
            icon_rect,
            icon,
            if hovered { p.text } else { p.muted },
            Size::Row.stroke(),
        );
        let font =
            egui::FontId::proportional(ui.style().text_styles[&egui::TextStyle::Button].size);
        ui.painter().text(
            Pos2::new(rect.left() + HEIGHT + 4.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            font.clone(),
            p.text,
        );
        if let Some(shortcut) = shortcut {
            ui.painter().text(
                Pos2::new(rect.right() - 6.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                shortcut,
                font,
                p.muted,
            );
        }
    }
    response
}

/// A compact button carrying an icon and a label. Used where the action is not
/// obvious from a glyph alone — upload and download mean different things
/// depending on which pane is in focus, so they keep their words.
pub fn icon_label_button(ui: &mut egui::Ui, icon: Icon, label: &str, p: Palette) -> egui::Response {
    let font = egui::FontId::proportional(ui.style().text_styles[&egui::TextStyle::Button].size);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, p.text);
    let pad = ui.spacing().button_padding;
    // The glyph keeps a margin inside the button rather than filling it, so the
    // stroke cannot reach the edge of the plate behind it.
    let glyph = (Size::Button.extent() - 4.0).max(8.0);
    let size = Vec2::new(
        pad.x * 2.0 + glyph + 6.0 + galley.rect.width(),
        (pad.y * 2.0 + galley.rect.height()).max(glyph + 6.0),
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        ui.painter()
            .rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
        let icon_rect = Rect::from_center_size(
            Pos2::new(rect.left() + pad.x + glyph * 0.5, rect.center().y),
            Vec2::splat(glyph),
        );
        draw(
            ui.painter(),
            icon_rect,
            icon,
            visuals.fg_stroke.color,
            Size::Button.stroke(),
        );
        ui.painter().galley(
            Pos2::new(
                icon_rect.right() + 6.0,
                rect.center().y - galley.rect.height() * 0.5,
            ),
            galley,
            visuals.fg_stroke.color,
        );
    }
    response
}

/// The shortcut badge: a small arrow in the icon's lower-left, the way every file
/// manager marks a link. Drawn over the glyph rather than beside it, so it costs
/// the row no width.
pub fn link_badge(painter: &egui::Painter, rect: Rect, color: Color32, width: f32) {
    let stroke = Stroke::new(width, color);
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.5;
    let p = |x: f32, y: f32| c + Vec2::new(x * r, y * r);
    // A shaft running up and to the right, with the head at its far end.
    let tip = (-0.15, 0.15);
    painter.line_segment([p(-0.85, 0.85), p(tip.0, tip.1)], stroke);
    painter.line_segment([p(tip.0, tip.1), p(tip.0, tip.1 + 0.45)], stroke);
    painter.line_segment([p(tip.0, tip.1), p(tip.0 - 0.45, tip.1)], stroke);
}

/// The application mark, wrapped for the window. The pixels themselves live in
/// `crate::appicon` so the build script can embed the same mark in the
/// executable without a second definition of it.
pub fn default_app_icon(size: u32) -> egui::IconData {
    egui::IconData {
        rgba: crate::appicon::pixels(size),
        width: size,
        height: size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant must paint something at every size. A silently empty arm is
    /// exactly the failure this module exists to prevent, and it is easy to write
    /// one by mistyping an offset.
    #[test]
    fn every_icon_paints_something_at_every_size() {
        const ALL: [Icon; 29] = [
            Icon::Terminal,
            Icon::Host,
            Icon::FolderUp,
            Icon::Home,
            Icon::Refresh,
            Icon::Group,
            Icon::Settings,
            Icon::Help,
            Icon::SplitHorizontal,
            Icon::SplitVertical,
            Icon::ClosePane,
            Icon::Restart,
            Icon::Search,
            Icon::Plus,
            Icon::ChevronLeft,
            Icon::ChevronRight,
            Icon::NewFolder,
            Icon::File,
            Icon::Close,
            Icon::Folder,
            Icon::FileText,
            Icon::FileCode,
            Icon::FileImage,
            Icon::FileArchive,
            Icon::FileMedia,
            Icon::FileBinary,
            Icon::Maximize,
            Icon::Restore,
            Icon::Minimize,
        ];
        for size in [Size::Row, Size::Button, Size::Title] {
            for icon in ALL {
                let ctx = egui::Context::default();
                let box_ = Rect::from_min_size(Pos2::ZERO, Vec2::splat(20.0));
                // Paint onto a bare layer, so the only shapes in the frame are the
                // icon's own — a panel background would mask an empty arm.
                let output = ctx.run(egui::RawInput::default(), |ctx| {
                    let painter =
                        egui::Painter::new(ctx.clone(), egui::LayerId::debug(), Rect::EVERYTHING);
                    draw(&painter, box_, icon, Color32::WHITE, size.stroke());
                });
                assert!(
                    !output.shapes.is_empty(),
                    "{icon:?} painted nothing at {size:?}"
                );
            }
        }
    }
}
