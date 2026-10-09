//! Hand-drawn UI glyphs.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::theme::Palette;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Terminal,
    Host,
    Connect,
    Reconnect,
    Disconnect,
    FolderUp,
    Home,
    Refresh,
    Group,
    Settings,
    Toolbox,
    Help,
    Info,
    Warning,
    SplitHorizontal,
    SplitVertical,
    ClosePane,
    Restart,
    History,
    Bell,
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
    Fullscreen,
    ExitFullscreen,
    Folder,
    FileText,
    FileCode,
    FileImage,
    FileArchive,
    FileMedia,
    FileBinary,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Size {
    Row,
    Button,
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
    pub fn button(self) -> Vec2 {
        match self {
            Size::Row => Vec2::new(20.0, 18.0),
            Size::Button => Vec2::new(24.0, 20.0),
            Size::Title => Vec2::new(40.0, 30.0),
        }
    }
}

pub fn draw(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32, width: f32) {
    if matches!(icon, Icon::Reconnect | Icon::Disconnect) {
        connection_glyph(painter, rect, icon, color, width);
        return;
    }
    let stroke = Stroke::new(width, color);
    let c = rect.center();
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
    let ring = |radius: f32, from: f32, to: f32| {
        let steps = 18;
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
    match icon {
        Icon::Reconnect | Icon::Disconnect => unreachable!(),
        Icon::Terminal => {
            boxed(-1.0, -0.85, 1.0, 0.85);
            seg((-0.55, -0.25), (-0.1, 0.05));
            seg((-0.1, 0.05), (-0.55, 0.35));
            seg((0.1, 0.4), (0.6, 0.4));
        }
        Icon::Host => {
            boxed(-1.0, -0.8, 1.0, 0.35);
            seg((0.0, 0.35), (0.0, 0.7));
            seg((-0.45, 0.8), (0.45, 0.8));
        }
        Icon::FolderUp | Icon::NewFolder => {
            seg((-1.0, 0.7), (-1.0, -0.7));
            seg((-1.0, -0.7), (-0.35, -0.7));
            seg((-0.35, -0.7), (-0.05, -0.35));
            seg((-0.05, -0.35), (1.0, -0.35));
            seg((1.0, -0.35), (1.0, 0.7));
            seg((-1.0, 0.7), (1.0, 0.7));
            if icon == Icon::FolderUp {
                seg((0.0, 0.45), (0.0, -0.3));
                seg((0.0, -0.3), (-0.27, -0.03));
                seg((0.0, -0.3), (0.27, -0.03));
            } else if icon == Icon::NewFolder {
                seg((0.0, 0.45), (0.0, 0.0));
                seg((-0.22, 0.22), (0.22, 0.22));
            }
        }
        Icon::Home => {
            seg((-0.95, -0.05), (0.0, -0.9));
            seg((0.0, -0.9), (0.95, -0.05));
            seg((-0.68, -0.28), (-0.68, 0.85));
            seg((0.68, -0.28), (0.68, 0.85));
            seg((-0.68, 0.85), (0.68, 0.85));
            boxed(-0.2, 0.18, 0.2, 0.85);
        }
        Icon::Refresh => {
            ring(0.78, -std::f32::consts::FRAC_PI_2, std::f32::consts::PI);
            seg((-0.78, 0.0), (-1.05, 0.32));
            seg((-0.78, 0.0), (-0.44, 0.28));
        }
        Icon::Connect => {
            boxed(-0.95, -0.85, 0.95, 0.30);
            seg((0.0, 0.30), (0.0, 0.72));
            seg((-0.40, 0.82), (0.40, 0.82));
            seg((-0.65, -0.28), (0.55, -0.28));
            seg((0.55, -0.28), (0.15, -0.60));
            seg((0.55, -0.28), (0.15, 0.04));
        }
        Icon::Group => {
            boxed(-1.0, -0.75, 0.45, 0.75);
            boxed(-0.45, -0.35, 1.0, 1.0);
        }
        Icon::Settings => {
            let teeth = 20;
            for step in 0..teeth {
                let a = step as f32 * std::f32::consts::TAU / teeth as f32;
                let b = (step + 1) as f32 * std::f32::consts::TAU / teeth as f32;
                let ra = if step % 2 == 0 { 0.93 } else { 0.72 };
                let rb = if (step + 1) % 2 == 0 { 0.93 } else { 0.72 };
                seg((ra * a.cos(), ra * a.sin()), (rb * b.cos(), rb * b.sin()));
            }
            painter.circle_stroke(c, r * 0.32, stroke);
        }
        Icon::Toolbox => {
            boxed(-0.9, -0.3, 0.9, 0.8);
            seg((-0.9, 0.05), (0.9, 0.05));
            seg((-0.35, -0.3), (-0.35, -0.7));
            seg((-0.35, -0.7), (0.35, -0.7));
            seg((0.35, -0.7), (0.35, -0.3));
            painter.rect_filled(
                Rect::from_center_size(p(0.0, 0.05), Vec2::splat(width * 2.0)),
                0,
                color,
            );
        }
        Icon::Help => {
            painter.circle_stroke(c, r * 0.88, stroke);
            seg((-0.32, -0.35), (-0.17, -0.55));
            seg((-0.17, -0.55), (0.18, -0.55));
            seg((0.18, -0.55), (0.38, -0.34));
            seg((0.38, -0.34), (0.3, -0.08));
            seg((0.3, -0.08), (0.0, 0.16));
            painter.circle_filled(p(0.0, 0.48), (width * 0.9).max(1.0), color);
        }
        Icon::Warning => {
            painter.add(egui::Shape::convex_polygon(
                vec![p(0.0, -1.0), p(1.0, 0.85), p(-1.0, 0.85)],
                color,
                Stroke::NONE,
            ));
            let ink = Color32::from_rgb(18, 29, 40);
            painter.line_segment([p(0.0, -0.4), p(0.0, 0.22)], Stroke::new(width * 1.35, ink));
            painter.circle_filled(p(0.0, 0.52), width, ink);
        }
        Icon::Info => {
            painter.circle_stroke(c, r * 0.88, stroke);
            seg((0.0, -0.5), (0.0, 0.15));
            painter.circle_filled(p(0.0, 0.48), (width * 0.9).max(1.0), color);
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
            boxed(-1.0, -0.8, 1.0, 0.8);
            seg((-0.45, -0.35), (0.05, 0.35));
            seg((0.05, -0.35), (-0.45, 0.35));
        }
        Icon::Restart => {
            ring(0.85, -1.05, 4.19);
            seg((0.0, -1.05), (0.0, -0.2));
        }
        Icon::History => {
            painter.circle_stroke(c, r * 0.82, stroke);
            seg((0.0, -0.48), (0.0, 0.0));
            seg((0.0, 0.0), (0.38, 0.23));
        }
        Icon::Bell => {
            ring(0.55, std::f32::consts::PI, std::f32::consts::TAU);
            seg((-0.62, 0.0), (0.62, 0.0));
            seg((0.0, -0.72), (0.0, -0.52));
            painter.circle_filled(p(0.0, 0.42), (width * 0.5).max(0.8), color);
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
            seg((-1.0, 0.7), (-1.0, -0.7));
            seg((-1.0, -0.7), (-0.35, -0.7));
            seg((-0.35, -0.7), (-0.05, -0.35));
            seg((-0.05, -0.35), (1.0, -0.35));
            seg((1.0, -0.35), (1.0, 0.7));
            seg((-1.0, 0.7), (1.0, 0.7));
        }
        Icon::Fullscreen | Icon::ExitFullscreen => {
            for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                let (corner, end) = if icon == Icon::Fullscreen {
                    (0.85, 0.35)
                } else {
                    (0.35, 0.85)
                };
                seg((x * corner, y * end), (x * corner, y * corner));
                seg((x * corner, y * corner), (x * end, y * corner));
            }
        }
        Icon::File
        | Icon::FileText
        | Icon::FileCode
        | Icon::FileImage
        | Icon::FileArchive
        | Icon::FileMedia
        | Icon::FileBinary => {
            seg((-0.7, -1.0), (0.35, -1.0));
            seg((0.35, -1.0), (0.8, -0.55));
            seg((0.8, -0.55), (0.8, 1.0));
            seg((0.8, 1.0), (-0.7, 1.0));
            seg((-0.7, 1.0), (-0.7, -1.0));
            seg((0.35, -1.0), (0.35, -0.55));
            seg((0.35, -0.55), (0.8, -0.55));
            match icon {
                Icon::File => {}
                Icon::FileText => {
                    for y in [0.15, 0.5, 0.85] {
                        seg((-0.45, y), (0.6, y));
                    }
                }
                Icon::FileCode => {
                    seg((-0.1, 0.2), (-0.5, 0.5));
                    seg((-0.5, 0.5), (-0.1, 0.8));
                    seg((0.25, 0.2), (0.65, 0.5));
                    seg((0.65, 0.5), (0.25, 0.8));
                }
                Icon::FileImage => {
                    seg((-0.45, 0.85), (-0.05, 0.35));
                    seg((-0.05, 0.35), (0.2, 0.65));
                    seg((0.2, 0.65), (0.4, 0.45));
                    seg((0.4, 0.45), (0.65, 0.85));
                    painter.circle_filled(p(0.35, 0.15), (width * 0.7).max(0.8), color);
                }
                Icon::FileArchive => {
                    for y in [0.1, 0.45, 0.8] {
                        seg((0.0, y), (0.3, y));
                    }
                    seg((0.15, 0.1), (0.15, 0.45));
                    seg((0.15, 0.45), (0.15, 0.8));
                }
                Icon::FileMedia => {
                    seg((-0.2, 0.2), (0.4, 0.5));
                    seg((0.4, 0.5), (-0.2, 0.8));
                    seg((-0.2, 0.8), (-0.2, 0.2));
                }
                Icon::FileBinary => {
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
            let d = 0.5;
            seg((-d, -d), (d, d));
            seg((d, -d), (-d, d));
        }
        Icon::Minimize => seg((-0.45, 0.0), (0.45, 0.0)),
        Icon::Maximize => boxed(-0.45, -0.45, 0.45, 0.45),
        Icon::Restore => {
            seg((-0.5, -0.5), (0.05, -0.5));
            seg((-0.5, -0.5), (-0.5, 0.05));
            boxed(-0.05, -0.05, 0.5, 0.5);
        }
    }
}

/// Native, rounded chain strokes with padding for the small status/tab buttons.
fn connection_glyph(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32, width: f32) {
    let extent = rect.width().min(rect.height());
    let stroke = Stroke::new(
        (width * 1.25).min(extent * 0.12),
        if icon == Icon::Disconnect {
            Color32::from_rgb(252, 49, 49)
        } else {
            color
        },
    );
    let radius = (extent * 0.5 - stroke.width).max(0.0);
    let point = |x, y| rect.center() + Vec2::new(x, y) * radius;
    let arc = |points: &mut Vec<Pos2>, x: f32, y: f32, r: f32, start: f32| {
        for step in 0..=16 {
            let angle = start + std::f32::consts::PI * step as f32 / 16.0;
            points.push(point(x + r * angle.cos(), y + r * angle.sin()));
        }
    };
    let rounded_path = |points: Vec<Pos2>| {
        if let (Some(first), Some(last)) = (points.first(), points.last()) {
            painter.circle_filled(*first, stroke.width * 0.5, stroke.color);
            painter.circle_filled(*last, stroke.width * 0.5, stroke.color);
        }
        painter.add(egui::Shape::line(points, stroke));
    };
    let half_pi = std::f32::consts::FRAC_PI_2;
    if icon == Icon::Reconnect {
        // Offset horizontal links, open at their overlap to keep them legible at 14 px.
        let mut upper = vec![point(-0.22, 0.20)];
        arc(&mut upper, -0.48, -0.22, 0.42, half_pi);
        arc(&mut upper, 0.16, -0.22, 0.42, -half_pi);
        upper.push(point(0.02, 0.20));
        rounded_path(upper);

        let mut lower = vec![point(0.22, -0.20)];
        arc(&mut lower, 0.48, 0.22, 0.42, -half_pi);
        arc(&mut lower, -0.16, 0.22, 0.42, half_pi);
        lower.push(point(-0.02, -0.20));
        rounded_path(lower);
    } else {
        // Two separated diagonal links crossed by a single, rounded slash.
        let rotate = |points: Vec<Pos2>| {
            points
                .into_iter()
                .map(|p| {
                    let v = p - rect.center();
                    rect.center()
                        + Vec2::new(v.x + v.y, v.y - v.x) * std::f32::consts::FRAC_1_SQRT_2
                })
                .collect()
        };
        let mut left = vec![point(-0.15, 0.27)];
        arc(&mut left, -0.48, 0.0, 0.27, half_pi);
        left.push(point(-0.15, -0.27));
        rounded_path(rotate(left));
        let mut right = vec![point(0.15, -0.27)];
        arc(&mut right, 0.48, 0.0, 0.27, -half_pi);
        right.push(point(0.15, 0.27));
        rounded_path(rotate(right));
        rounded_path(vec![point(-0.65, -0.65), point(0.65, 0.65)]);
    }
}

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

pub fn icon_row(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    shortcut: Option<&str>,
    p: Palette,
) -> egui::Response {
    icon_row_inner(ui, icon, label, shortcut, p, false)
}

/// Full-width navigation row; selection follows the clicked list entry.
pub fn navigation_row(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    selected: bool,
    p: Palette,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), 24.0),
        Sense::click_and_drag(),
    );
    if ui.is_rect_visible(rect) {
        let selected_fade =
            ui.ctx()
                .animate_bool_with_time(response.id.with("selected"), selected, 0.12);
        let hover_fade =
            ui.ctx()
                .animate_bool_with_time(response.id.with("hover"), response.hovered(), 0.10);
        if selected_fade > 0.0 || hover_fade > 0.0 {
            ui.painter().rect_filled(
                rect,
                5,
                p.panel
                    .lerp_to_gamma(p.raised, selected_fade.max(hover_fade * 0.55)),
            );
        }
        if selected_fade > 0.0 {
            ui.painter().rect_filled(
                Rect::from_min_max(rect.left_top(), Pos2::new(rect.left() + 3.0, rect.bottom())),
                2,
                p.accent.gamma_multiply(selected_fade),
            );
        }
        draw(
            ui.painter(),
            Rect::from_center_size(
                Pos2::new(rect.left() + 13.0, rect.center().y),
                Vec2::splat(15.0),
            ),
            icon,
            if selected { p.text } else { p.muted },
            1.3,
        );
        let mut job = egui::text::LayoutJob::simple(
            label.to_owned(),
            egui::FontId::proportional(14.0),
            p.text,
            (rect.width() - 34.0).max(0.0),
        );
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        let galley = ui.painter().layout_job(job);
        ui.painter().with_clip_rect(rect).galley(
            Pos2::new(rect.left() + 30.0, rect.center().y - galley.size().y * 0.5),
            galley,
            p.text,
        );
    }
    response
}

pub fn icon_row_primary(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    shortcut: Option<&str>,
    p: Palette,
) -> egui::Response {
    icon_row_inner(ui, icon, label, shortcut, p, true)
}

fn icon_row_inner(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    shortcut: Option<&str>,
    p: Palette,
    primary: bool,
) -> egui::Response {
    let height = if primary { 28.0 } else { 22.0 };
    let width = (ui.available_width() - 8.0).clamp(180.0, 280.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        if hovered || primary {
            let visuals = ui.style().interact_selectable(&response, false);
            ui.painter().rect_filled(
                rect,
                ui.style().visuals.widgets.inactive.corner_radius,
                if hovered {
                    visuals.bg_fill
                } else {
                    p.accent.gamma_multiply(0.12)
                },
            );
        }
        let icon_rect = Rect::from_center_size(
            Pos2::new(rect.left() + height * 0.5, rect.center().y),
            Vec2::splat(16.0),
        );
        draw(
            ui.painter(),
            icon_rect,
            icon,
            if primary {
                p.accent
            } else if hovered {
                p.text
            } else {
                p.muted
            },
            Size::Row.stroke(),
        );
        let font =
            egui::FontId::proportional(ui.style().text_styles[&egui::TextStyle::Button].size);
        ui.painter().text(
            Pos2::new(rect.left() + height + 4.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            font.clone(),
            if primary { p.accent } else { p.text },
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

pub fn icon_label_button(ui: &mut egui::Ui, icon: Icon, label: &str, p: Palette) -> egui::Response {
    let font = egui::FontId::proportional(ui.style().text_styles[&egui::TextStyle::Button].size);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, p.text);
    let pad = ui.spacing().button_padding;
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

pub fn link_badge(painter: &egui::Painter, rect: Rect, color: Color32, width: f32) {
    let stroke = Stroke::new(width, color);
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.5;
    let p = |x: f32, y: f32| c + Vec2::new(x * r, y * r);
    let tip = (-0.15, 0.15);
    painter.line_segment([p(-0.85, 0.85), p(tip.0, tip.1)], stroke);
    painter.line_segment([p(tip.0, tip.1), p(tip.0, tip.1 + 0.45)], stroke);
    painter.line_segment([p(tip.0, tip.1), p(tip.0 - 0.45, tip.1)], stroke);
}

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

    #[test]
    fn connection_icons_use_native_shapes_within_their_buttons() {
        for icon in [Icon::Reconnect, Icon::Disconnect] {
            for size in [Vec2::splat(14.0), Size::Row.button(), Size::Button.button()] {
                let ctx = egui::Context::default();
                let rect = Rect::from_min_size(Pos2::new(20.0, 20.0), size);
                let output = ctx.run(egui::RawInput::default(), |ctx| {
                    let painter =
                        egui::Painter::new(ctx.clone(), egui::LayerId::debug(), Rect::EVERYTHING);
                    draw(&painter, rect, icon, Color32::GRAY, Size::Row.stroke());
                });
                assert!(!output.shapes.is_empty());
                for clipped in output.shapes {
                    assert!(
                        matches!(clipped.shape, egui::Shape::Path(_) | egui::Shape::Circle(_)),
                        "{icon:?} must render without an image texture"
                    );
                    assert!(
                        rect.contains_rect(clipped.shape.visual_bounding_rect()),
                        "{icon:?} draws outside its button at {size:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn every_icon_paints_something_at_every_size() {
        const ALL: [Icon; 39] = [
            Icon::Terminal,
            Icon::Host,
            Icon::Connect,
            Icon::Reconnect,
            Icon::Disconnect,
            Icon::FolderUp,
            Icon::Home,
            Icon::Refresh,
            Icon::Group,
            Icon::Settings,
            Icon::Toolbox,
            Icon::Help,
            Icon::Info,
            Icon::Warning,
            Icon::SplitHorizontal,
            Icon::SplitVertical,
            Icon::ClosePane,
            Icon::Restart,
            Icon::History,
            Icon::Bell,
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
            Icon::Fullscreen,
            Icon::ExitFullscreen,
        ];
        for size in [Size::Row, Size::Button, Size::Title] {
            for icon in ALL {
                let ctx = egui::Context::default();
                let box_ = Rect::from_min_size(Pos2::ZERO, Vec2::splat(20.0));
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
