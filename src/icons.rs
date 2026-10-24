//! Hand-drawn UI glyphs.
//!
//! Every glyph is drawn on the same 24-unit grid (the convention of the common
//! line-icon sets), with rounded corners, smooth arcs and round line ends, so
//! the set reads as one family at 14–18 px in both themes. Nothing here is a
//! texture: each icon is a few vector shapes in the caller's colour.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::theme::Palette;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Terminal,
    /// An SSH host: a server.
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
    ChevronDown,
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
    /// A serial port: a plug.
    Serial,
    NewWindow,
    Sidebar,
    Keyboard,
    Sun,
    Moon,
    Power,
    User,
    Lock,
    Key,
    ArrowUp,
    ArrowDown,
    Forward,
    Copy,
    Check,
    Trash,
    Pause,
    Play,
    Edit,
}

impl Icon {
    pub const ALL: [Icon; 59] = [
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
        Icon::ChevronDown,
        Icon::NewFolder,
        Icon::File,
        Icon::Close,
        Icon::Maximize,
        Icon::Restore,
        Icon::Minimize,
        Icon::Fullscreen,
        Icon::ExitFullscreen,
        Icon::Folder,
        Icon::FileText,
        Icon::FileCode,
        Icon::FileImage,
        Icon::FileArchive,
        Icon::FileMedia,
        Icon::FileBinary,
        Icon::Serial,
        Icon::NewWindow,
        Icon::Sidebar,
        Icon::Keyboard,
        Icon::Sun,
        Icon::Moon,
        Icon::Power,
        Icon::User,
        Icon::Lock,
        Icon::Key,
        Icon::ArrowUp,
        Icon::ArrowDown,
        Icon::Forward,
        Icon::Copy,
        Icon::Check,
        Icon::Trash,
        Icon::Pause,
        Icon::Play,
        Icon::Edit,
    ];
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Size {
    Row,
    Button,
    Title,
}

impl Size {
    /// The glyph's square, centred in [`Self::button`].
    pub fn glyph(self, button: Rect) -> Rect {
        Rect::from_center_size(button.center(), Vec2::splat(self.extent()))
    }
    fn extent(self) -> f32 {
        match self {
            Size::Row => 16.0,
            Size::Button => 18.0,
            Size::Title => 24.0,
        }
    }
    pub fn stroke(self) -> f32 {
        match self {
            Size::Row => 1.3,
            Size::Button => 1.4,
            Size::Title => 1.1,
        }
    }
    pub fn button(self) -> Vec2 {
        match self {
            Size::Row => Vec2::new(24.0, 22.0),
            Size::Button => Vec2::new(28.0, 26.0),
            Size::Title => Vec2::new(44.0, 32.0),
        }
    }
}

/// Draws on the 24-unit grid centred in `rect`.
struct Pen<'a> {
    painter: &'a egui::Painter,
    origin: Pos2,
    unit: f32,
    stroke: Stroke,
}

impl Pen<'_> {
    fn at(&self, x: f32, y: f32) -> Pos2 {
        self.origin + Vec2::new(x, y) * self.unit
    }

    /// An open polyline with round ends.
    fn line(&self, points: &[(f32, f32)]) {
        let points: Vec<Pos2> = points.iter().map(|&(x, y)| self.at(x, y)).collect();
        self.path(points);
    }

    fn path(&self, points: Vec<Pos2>) {
        let cap = self.stroke.width * 0.5;
        if let (Some(first), Some(last)) = (points.first(), points.last()) {
            self.painter.circle_filled(*first, cap, self.stroke.color);
            self.painter.circle_filled(*last, cap, self.stroke.color);
        }
        self.painter.add(egui::Shape::line(points, self.stroke));
    }

    /// A closed outline.
    fn closed(&self, points: &[(f32, f32)]) {
        let points: Vec<Pos2> = points.iter().map(|&(x, y)| self.at(x, y)).collect();
        self.painter
            .add(egui::Shape::closed_line(points, self.stroke));
    }

    fn filled(&self, points: &[(f32, f32)]) {
        let points: Vec<Pos2> = points.iter().map(|&(x, y)| self.at(x, y)).collect();
        self.painter.add(egui::Shape::convex_polygon(
            points,
            self.stroke.color,
            Stroke::NONE,
        ));
    }

    /// A rounded rectangle outline, corners given in grid units.
    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32, radius: f32) {
        self.painter.rect_stroke(
            Rect::from_min_max(self.at(x0, y0), self.at(x1, y1)),
            radius * self.unit,
            self.stroke,
            StrokeKind::Middle,
        );
    }

    fn circle(&self, x: f32, y: f32, r: f32) {
        self.painter
            .circle_stroke(self.at(x, y), r * self.unit, self.stroke);
    }

    fn dot(&self, x: f32, y: f32, r: f32) {
        self.painter
            .circle_filled(self.at(x, y), r * self.unit, self.stroke.color);
    }

    /// Points along an arc, angles in degrees, clockwise on screen.
    fn arc_points(&self, x: f32, y: f32, r: f32, from: f32, to: f32) -> Vec<Pos2> {
        let steps = (((to - from).abs() / 12.0).ceil() as usize).max(4);
        (0..=steps)
            .map(|step| {
                let angle = (from + (to - from) * step as f32 / steps as f32).to_radians();
                self.at(x + r * angle.cos(), y + r * angle.sin())
            })
            .collect()
    }

    fn arc(&self, x: f32, y: f32, r: f32, from: f32, to: f32) {
        self.path(self.arc_points(x, y, r, from, to));
    }

    /// A folder outline; the shared base of the folder glyphs.
    fn folder(&self) {
        self.closed(&[
            (2.5, 18.0),
            (2.5, 6.0),
            (3.5, 5.0),
            (8.8, 5.0),
            (10.8, 7.5),
            (20.5, 7.5),
            (21.5, 8.5),
            (21.5, 18.0),
            (20.5, 19.0),
            (3.5, 19.0),
        ]);
    }

    /// A page with a folded corner; the shared base of the file glyphs.
    fn page(&self) {
        self.closed(&[
            (5.0, 3.5),
            (14.0, 2.5),
            (19.5, 8.0),
            (19.5, 20.5),
            (18.5, 21.5),
            (5.5, 21.5),
            (4.5, 20.5),
            (4.5, 3.5),
        ]);
        self.line(&[(14.0, 2.5), (14.0, 8.0), (19.5, 8.0)]);
    }

    /// A rounded window frame; the base of the layout glyphs.
    fn frame(&self) {
        self.rect(3.0, 4.0, 21.0, 20.0, 2.5);
    }
}

pub fn draw(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32, width: f32) {
    let extent = rect.width().min(rect.height());
    let unit = extent / 24.0;
    let pen = Pen {
        painter,
        origin: rect.center() - Vec2::splat(12.0 * unit),
        unit,
        stroke: Stroke::new(width, color),
    };
    match icon {
        Icon::Terminal => {
            pen.rect(2.5, 4.0, 21.5, 20.0, 2.5);
            pen.line(&[(6.5, 9.0), (10.0, 12.0), (6.5, 15.0)]);
            pen.line(&[(12.5, 15.0), (17.5, 15.0)]);
        }
        Icon::Host => {
            pen.rect(3.0, 3.5, 21.0, 10.5, 2.0);
            pen.rect(3.0, 13.5, 21.0, 20.5, 2.0);
            pen.dot(7.0, 7.0, 1.1);
            pen.dot(7.0, 17.0, 1.1);
            pen.line(&[(11.0, 7.0), (17.0, 7.0)]);
            pen.line(&[(11.0, 17.0), (17.0, 17.0)]);
        }
        Icon::Serial => {
            pen.line(&[(9.0, 2.5), (9.0, 7.0)]);
            pen.line(&[(15.0, 2.5), (15.0, 7.0)]);
            pen.closed(&[
                (5.5, 7.0),
                (18.5, 7.0),
                (18.5, 11.0),
                (15.5, 15.0),
                (8.5, 15.0),
                (5.5, 11.0),
            ]);
            pen.line(&[(12.0, 15.0), (12.0, 21.5)]);
        }
        Icon::Connect => {
            pen.line(&[
                (14.5, 3.5),
                (18.5, 3.5),
                (20.5, 5.5),
                (20.5, 18.5),
                (18.5, 20.5),
                (14.5, 20.5),
            ]);
            pen.line(&[(9.5, 7.5), (14.0, 12.0), (9.5, 16.5)]);
            pen.line(&[(14.0, 12.0), (3.5, 12.0)]);
        }
        Icon::Reconnect | Icon::Refresh => {
            // A clockwise arrow: reconnecting is the same gesture as reloading.
            let mut arc = pen.arc_points(12.0, 12.0, 8.5, 0.0, 318.0);
            arc.push(pen.at(20.5, 8.5));
            pen.path(arc);
            pen.line(&[(20.5, 3.5), (20.5, 8.5), (15.5, 8.5)]);
        }
        Icon::Restart => {
            let mut arc = pen.arc_points(12.0, 12.0, 8.5, 180.0, -138.0);
            arc.push(pen.at(3.5, 8.5));
            pen.path(arc);
            pen.line(&[(3.5, 3.5), (3.5, 8.5), (8.5, 8.5)]);
        }
        Icon::Disconnect => {
            // The plug, pulled: prongs and body offset, crossed by a slash.
            pen.line(&[(9.5, 3.0), (9.5, 6.5)]);
            pen.line(&[(14.5, 3.0), (14.5, 6.5)]);
            pen.closed(&[
                (6.5, 6.5),
                (17.5, 6.5),
                (17.5, 10.0),
                (15.0, 13.0),
                (9.0, 13.0),
                (6.5, 10.0),
            ]);
            pen.line(&[(12.0, 13.0), (12.0, 16.0)]);
            pen.line(&[(9.0, 19.5), (15.0, 19.5)]);
            pen.line(&[(3.5, 3.5), (20.5, 20.5)]);
        }
        Icon::Folder => pen.folder(),
        Icon::FolderUp => {
            pen.folder();
            pen.line(&[(12.0, 16.0), (12.0, 10.5)]);
            pen.line(&[(9.5, 13.0), (12.0, 10.5), (14.5, 13.0)]);
        }
        Icon::NewFolder => {
            pen.folder();
            pen.line(&[(12.0, 10.5), (12.0, 16.0)]);
            pen.line(&[(9.25, 13.25), (14.75, 13.25)]);
        }
        Icon::Home => {
            pen.line(&[(3.0, 11.0), (12.0, 3.5), (21.0, 11.0)]);
            pen.line(&[
                (5.5, 9.0),
                (5.5, 19.5),
                (6.5, 20.5),
                (17.5, 20.5),
                (18.5, 19.5),
                (18.5, 9.0),
            ]);
            pen.line(&[(10.0, 20.5), (10.0, 14.5), (14.0, 14.5), (14.0, 20.5)]);
        }
        Icon::Group => {
            pen.closed(&[(12.0, 3.0), (21.0, 7.75), (12.0, 12.5), (3.0, 7.75)]);
            pen.line(&[(3.0, 12.25), (12.0, 17.0), (21.0, 12.25)]);
            pen.line(&[(3.0, 16.75), (12.0, 21.5), (21.0, 16.75)]);
        }
        Icon::Settings => {
            // Eight rounded teeth around a hub.
            let teeth = 8;
            let mut points = Vec::new();
            for tooth in 0..teeth {
                let base = tooth as f32 * 360.0 / teeth as f32;
                for (offset, radius) in [(-17.0, 7.2), (-10.0, 9.6), (10.0, 9.6), (17.0, 7.2)] {
                    let angle = (base + offset).to_radians();
                    points.push((12.0 + radius * angle.cos(), 12.0 + radius * angle.sin()));
                }
            }
            pen.closed(&points);
            pen.circle(12.0, 12.0, 3.0);
        }
        Icon::Toolbox => {
            pen.rect(3.0, 8.5, 21.0, 20.0, 2.0);
            pen.line(&[
                (9.0, 8.5),
                (9.0, 6.0),
                (10.0, 5.0),
                (14.0, 5.0),
                (15.0, 6.0),
                (15.0, 8.5),
            ]);
            pen.line(&[(3.0, 13.5), (10.5, 13.5)]);
            pen.line(&[(13.5, 13.5), (21.0, 13.5)]);
            pen.rect(10.5, 12.0, 13.5, 15.0, 0.8);
        }
        Icon::Help => {
            pen.circle(12.0, 12.0, 9.5);
            pen.arc(12.0, 9.2, 2.8, 200.0, 390.0);
            pen.line(&[(14.4, 10.6), (12.0, 12.4), (12.0, 13.8)]);
            pen.dot(12.0, 17.2, 1.15);
        }
        Icon::Info => {
            pen.circle(12.0, 12.0, 9.5);
            pen.line(&[(12.0, 11.0), (12.0, 16.5)]);
            pen.dot(12.0, 7.8, 1.15);
        }
        Icon::Warning => {
            pen.filled(&[(12.0, 2.5), (22.0, 20.5), (2.0, 20.5)]);
            let ink = Color32::from_rgb(18, 29, 40);
            let mark = Pen {
                stroke: Stroke::new(width * 1.3, ink),
                ..pen
            };
            mark.line(&[(12.0, 9.0), (12.0, 14.0)]);
            mark.dot(12.0, 17.2, 1.2);
        }
        Icon::SplitHorizontal => {
            pen.frame();
            pen.line(&[(12.0, 4.0), (12.0, 20.0)]);
        }
        Icon::SplitVertical => {
            pen.frame();
            pen.line(&[(3.0, 12.0), (21.0, 12.0)]);
        }
        Icon::ClosePane => {
            pen.frame();
            pen.line(&[(9.5, 9.5), (14.5, 14.5)]);
            pen.line(&[(14.5, 9.5), (9.5, 14.5)]);
        }
        Icon::Sidebar => {
            pen.frame();
            pen.line(&[(9.0, 4.0), (9.0, 20.0)]);
        }
        Icon::NewWindow => {
            pen.frame();
            pen.line(&[(3.0, 8.5), (21.0, 8.5)]);
            pen.line(&[(12.0, 11.5), (12.0, 17.5)]);
            pen.line(&[(9.0, 14.5), (15.0, 14.5)]);
        }
        Icon::History => {
            let mut arc = pen.arc_points(12.0, 12.0, 8.5, 180.0, -138.0);
            arc.push(pen.at(3.5, 8.5));
            pen.path(arc);
            pen.line(&[(3.5, 3.5), (3.5, 8.5), (8.5, 8.5)]);
            pen.line(&[(12.0, 7.5), (12.0, 12.0), (15.5, 14.0)]);
        }
        Icon::Bell => {
            let mut outline = vec![pen.at(3.5, 17.0), pen.at(5.0, 15.0), pen.at(6.0, 11.5)];
            outline.extend(pen.arc_points(12.0, 10.0, 6.0, 195.0, 345.0));
            outline.extend([pen.at(18.0, 11.5), pen.at(19.0, 15.0), pen.at(20.5, 17.0)]);
            outline.push(pen.at(3.5, 17.0));
            pen.painter.add(egui::Shape::line(outline, pen.stroke));
            pen.arc(12.0, 19.0, 2.2, 0.0, 180.0);
        }
        Icon::Search => {
            pen.circle(10.5, 10.5, 6.5);
            pen.line(&[(15.3, 15.3), (20.5, 20.5)]);
        }
        Icon::Plus => {
            pen.line(&[(5.0, 12.0), (19.0, 12.0)]);
            pen.line(&[(12.0, 5.0), (12.0, 19.0)]);
        }
        Icon::Check => pen.line(&[(5.0, 12.5), (10.0, 17.5), (19.5, 7.0)]),
        Icon::ChevronLeft => pen.line(&[(14.5, 6.0), (8.5, 12.0), (14.5, 18.0)]),
        Icon::ChevronRight => pen.line(&[(9.5, 6.0), (15.5, 12.0), (9.5, 18.0)]),
        Icon::ChevronDown => pen.line(&[(6.0, 9.5), (12.0, 15.5), (18.0, 9.5)]),
        Icon::File
        | Icon::FileText
        | Icon::FileCode
        | Icon::FileImage
        | Icon::FileArchive
        | Icon::FileMedia
        | Icon::FileBinary => {
            pen.page();
            match icon {
                Icon::FileText => {
                    pen.line(&[(8.0, 12.5), (16.0, 12.5)]);
                    pen.line(&[(8.0, 16.5), (14.0, 16.5)]);
                }
                Icon::FileCode => {
                    pen.line(&[(10.0, 11.5), (7.5, 14.5), (10.0, 17.5)]);
                    pen.line(&[(14.0, 11.5), (16.5, 14.5), (14.0, 17.5)]);
                }
                Icon::FileImage => {
                    pen.dot(9.5, 11.5, 1.3);
                    pen.line(&[
                        (7.0, 18.5),
                        (11.0, 14.0),
                        (13.5, 16.5),
                        (15.0, 15.0),
                        (17.0, 18.5),
                    ]);
                }
                Icon::FileArchive => {
                    for y in [6.0, 9.5, 13.0] {
                        pen.line(&[(9.5, y), (11.0, y)]);
                    }
                    pen.rect(9.0, 15.0, 12.0, 19.0, 0.8);
                }
                Icon::FileMedia => {
                    pen.closed(&[(10.0, 11.5), (15.5, 14.75), (10.0, 18.0)]);
                }
                Icon::FileBinary => {
                    pen.rect(8.0, 12.0, 10.5, 17.5, 1.2);
                    pen.line(&[(14.0, 12.0), (15.0, 11.5), (15.0, 17.5)]);
                }
                _ => {}
            }
        }
        Icon::Close => {
            pen.line(&[(6.5, 6.5), (17.5, 17.5)]);
            pen.line(&[(17.5, 6.5), (6.5, 17.5)]);
        }
        Icon::Minimize => pen.line(&[(7.0, 12.0), (17.0, 12.0)]),
        Icon::Maximize => pen.rect(7.0, 7.0, 17.0, 17.0, 1.5),
        Icon::Restore => {
            pen.rect(6.5, 9.5, 14.5, 17.5, 1.5);
            pen.line(&[
                (9.5, 9.5),
                (9.5, 7.5),
                (10.5, 6.5),
                (16.5, 6.5),
                (17.5, 7.5),
                (17.5, 13.5),
                (16.5, 14.5),
                (14.5, 14.5),
            ]);
        }
        Icon::Fullscreen => {
            pen.line(&[(3.5, 8.5), (3.5, 3.5), (8.5, 3.5)]);
            pen.line(&[(15.5, 3.5), (20.5, 3.5), (20.5, 8.5)]);
            pen.line(&[(20.5, 15.5), (20.5, 20.5), (15.5, 20.5)]);
            pen.line(&[(8.5, 20.5), (3.5, 20.5), (3.5, 15.5)]);
        }
        Icon::ExitFullscreen => {
            pen.line(&[(3.5, 8.5), (8.5, 8.5), (8.5, 3.5)]);
            pen.line(&[(15.5, 3.5), (15.5, 8.5), (20.5, 8.5)]);
            pen.line(&[(20.5, 15.5), (15.5, 15.5), (15.5, 20.5)]);
            pen.line(&[(8.5, 20.5), (8.5, 15.5), (3.5, 15.5)]);
        }
        Icon::Keyboard => {
            pen.rect(2.0, 5.5, 22.0, 18.5, 2.5);
            for x in [6.0, 10.0, 14.0, 18.0] {
                pen.dot(x, 9.5, 0.9);
            }
            for x in [8.0, 12.0, 16.0] {
                pen.dot(x, 12.5, 0.9);
            }
            pen.line(&[(8.0, 15.5), (16.0, 15.5)]);
        }
        Icon::Sun => {
            pen.circle(12.0, 12.0, 4.0);
            for step in 0..8 {
                let angle = (step as f32 * 45.0).to_radians();
                let (c, s) = (angle.cos(), angle.sin());
                pen.line(&[
                    (12.0 + 7.0 * c, 12.0 + 7.0 * s),
                    (12.0 + 9.0 * c, 12.0 + 9.0 * s),
                ]);
            }
        }
        Icon::Moon => {
            // Two separate arcs: joined at the tips, the sharp cusps would be
            // mitred into spikes.
            pen.arc(12.0, 12.0, 9.0, -90.0, -360.0);
            pen.arc(16.5, 7.5, 6.364, 45.0, 225.0);
        }
        Icon::Power => {
            pen.arc(12.0, 13.0, 8.0, -55.0, 235.0);
            pen.line(&[(12.0, 2.5), (12.0, 11.0)]);
        }
        Icon::User => {
            pen.circle(12.0, 8.0, 4.0);
            pen.arc(12.0, 21.5, 8.0, 180.0, 360.0);
        }
        Icon::Lock => {
            pen.rect(4.5, 11.0, 19.5, 21.0, 2.0);
            let mut shackle = vec![pen.at(8.0, 11.0)];
            shackle.extend(pen.arc_points(12.0, 7.5, 4.0, 180.0, 360.0));
            shackle.push(pen.at(16.0, 11.0));
            pen.path(shackle);
            pen.dot(12.0, 16.0, 1.2);
        }
        Icon::Key => {
            pen.circle(7.5, 16.5, 4.5);
            pen.line(&[(10.7, 13.3), (20.5, 3.5)]);
            pen.line(&[(17.0, 7.0), (19.5, 9.5)]);
            pen.line(&[(14.5, 9.5), (16.5, 11.5)]);
        }
        Icon::ArrowUp => {
            pen.line(&[(12.0, 19.5), (12.0, 4.5)]);
            pen.line(&[(6.0, 10.5), (12.0, 4.5), (18.0, 10.5)]);
        }
        Icon::ArrowDown => {
            pen.line(&[(12.0, 4.5), (12.0, 19.5)]);
            pen.line(&[(6.0, 13.5), (12.0, 19.5), (18.0, 13.5)]);
        }
        Icon::Forward => {
            pen.line(&[(16.5, 3.5), (20.5, 7.5), (16.5, 11.5)]);
            pen.line(&[(20.5, 7.5), (4.0, 7.5)]);
            pen.line(&[(7.5, 12.5), (3.5, 16.5), (7.5, 20.5)]);
            pen.line(&[(3.5, 16.5), (20.0, 16.5)]);
        }
        Icon::Edit => {
            pen.closed(&[
                (16.5, 3.5),
                (20.5, 7.5),
                (8.0, 20.0),
                (3.5, 20.5),
                (4.0, 16.0),
            ]);
            pen.line(&[(14.0, 6.0), (18.0, 10.0)]);
        }
        Icon::Pause => {
            pen.rect(6.0, 4.5, 10.0, 19.5, 1.2);
            pen.rect(14.0, 4.5, 18.0, 19.5, 1.2);
        }
        Icon::Play => pen.closed(&[(7.0, 4.5), (19.0, 12.0), (7.0, 19.5)]),
        Icon::Trash => {
            pen.line(&[(3.5, 6.0), (20.5, 6.0)]);
            pen.line(&[
                (9.0, 6.0),
                (9.0, 4.0),
                (10.0, 3.0),
                (14.0, 3.0),
                (15.0, 4.0),
                (15.0, 6.0),
            ]);
            pen.line(&[
                (5.5, 6.0),
                (6.5, 20.0),
                (7.5, 21.0),
                (16.5, 21.0),
                (17.5, 20.0),
                (18.5, 6.0),
            ]);
            pen.line(&[(10.0, 10.5), (10.0, 16.5)]);
            pen.line(&[(14.0, 10.5), (14.0, 16.5)]);
        }
        Icon::Copy => {
            pen.rect(8.5, 8.5, 21.0, 21.0, 2.0);
            pen.line(&[
                (5.0, 15.5),
                (4.0, 15.5),
                (3.0, 14.5),
                (3.0, 4.0),
                (4.0, 3.0),
                (14.5, 3.0),
                (15.5, 4.0),
                (15.5, 5.0),
            ]);
        }
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
    draw(
        ui.painter(),
        Size::Title.glyph(rect),
        icon,
        color,
        Size::Title.stroke(),
    );
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
        size.glyph(rect),
        icon,
        if hovered { p.text } else { p.muted },
        size.stroke(),
    );
    response
}

/// The small × on a tab.
pub fn close_button(ui: &mut egui::Ui, p: Palette, tip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        if hovered {
            ui.painter().circle_filled(rect.center(), 8.5, p.raised);
        }
        draw(
            ui.painter(),
            rect.shrink(4.5),
            Icon::Close,
            if hovered { p.text } else { p.muted },
            1.4,
        );
    }
    response.on_hover_text(tip)
}

/// An icon button that shows whether its setting is on.
pub fn icon_toggle(ui: &mut egui::Ui, icon: Icon, on: bool, p: Palette) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Size::Button.button(), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        if on || hovered {
            ui.painter().rect_filled(
                rect,
                ui.style().visuals.widgets.inactive.corner_radius,
                if on {
                    p.accent.gamma_multiply(0.16)
                } else {
                    ui.style().interact_selectable(&response, false).bg_fill
                },
            );
        }
        draw(
            ui.painter(),
            Size::Button.glyph(rect),
            icon,
            if on {
                p.accent
            } else if hovered {
                p.text
            } else {
                p.muted
            },
            Size::Button.stroke(),
        );
    }
    response
}

/// A square launcher: a large icon over a two-character caption.
pub fn tile(ui: &mut egui::Ui, icon: Icon, caption: &str, p: Palette) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(68.0, 56.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let fade: f32 = if hovered { 1.0 } else { 0.0 };
        ui.painter()
            .rect_filled(rect, 8, p.raised.gamma_multiply(0.45 + 0.55 * fade));
        draw(
            ui.painter(),
            Rect::from_center_size(rect.center() - Vec2::new(0.0, 8.0), Vec2::splat(20.0)),
            icon,
            if hovered { p.accent } else { p.text },
            1.5,
        );
        ui.painter().text(
            rect.center() + Vec2::new(0.0, 14.0),
            egui::Align2::CENTER_CENTER,
            caption,
            egui::FontId::proportional(12.0),
            if hovered { p.text } else { p.muted },
        );
    }
    response
}

pub fn icon_row(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    shortcut: Option<&str>,
    p: Palette,
) -> egui::Response {
    icon_row_inner(ui, icon, label, shortcut, p)
}

/// Full-width navigation row; selection follows the clicked list entry.
pub fn navigation_row(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    selected: bool,
    tag: Option<Color32>,
    p: Palette,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), 24.0),
        Sense::click_and_drag(),
    );
    if ui.is_rect_visible(rect) {
        // Feedback follows the pointer at once: a fade-in reads as lag.
        let selected_fade: f32 = if selected { 1.0 } else { 0.0 };
        let hover_fade: f32 = if response.hovered() { 1.0 } else { 0.0 };
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
        // The connection's tag colour, as a dot at the end of the row.
        let text_right = if let Some(tag) = tag {
            ui.painter()
                .circle_filled(Pos2::new(rect.right() - 9.0, rect.center().y), 3.5, tag);
            rect.right() - 18.0
        } else {
            rect.right() - 4.0
        };
        let mut job = egui::text::LayoutJob::simple(
            label.to_owned(),
            egui::FontId::proportional(14.0),
            p.text,
            (text_right - rect.left() - 30.0).max(0.0),
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

fn icon_row_inner(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    shortcut: Option<&str>,
    p: Palette,
) -> egui::Response {
    let height = 26.0;
    let width = ui.available_width().clamp(180.0, 300.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let enabled = ui.is_enabled();
        if hovered && enabled {
            ui.painter().rect_filled(
                rect,
                6,
                ui.style().interact_selectable(&response, false).bg_fill,
            );
        }
        draw(
            ui.painter(),
            Rect::from_center_size(
                Pos2::new(rect.left() + 14.0, rect.center().y),
                Vec2::splat(16.0),
            ),
            icon,
            if hovered && enabled {
                p.accent
            } else {
                p.muted
            },
            Size::Row.stroke(),
        );
        let font = egui::FontId::proportional(14.0);
        ui.painter().text(
            Pos2::new(rect.left() + 32.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            font,
            if enabled { p.text } else { p.muted },
        );
        if let Some(shortcut) = shortcut {
            ui.painter().text(
                Pos2::new(rect.right() - 8.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                shortcut,
                egui::FontId::proportional(12.0),
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
        for size in [Size::Row, Size::Button, Size::Title] {
            for icon in Icon::ALL {
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
