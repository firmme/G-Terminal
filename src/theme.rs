use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, Stroke};

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub panel: Color32,
    pub raised: Color32,
    pub line: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
}

impl Palette {
    pub fn new(light: bool) -> Self {
        if light {
            Self {
                bg: Color32::from_rgb(246, 248, 251),
                panel: Color32::from_rgb(235, 240, 246),
                raised: Color32::WHITE,
                line: Color32::from_rgb(211, 221, 231),
                text: Color32::from_rgb(32, 44, 63),
                muted: Color32::from_rgb(93, 111, 134),
                accent: Color32::from_rgb(0, 119, 106),
            }
        } else {
            Self {
                bg: Color32::from_rgb(13, 18, 26),
                panel: Color32::from_rgb(19, 26, 36),
                raised: Color32::from_rgb(28, 38, 51),
                line: Color32::from_rgb(37, 49, 64),
                text: Color32::from_rgb(216, 226, 238),
                muted: Color32::from_rgb(124, 144, 167),
                accent: Color32::from_rgb(92, 224, 184),
            }
        }
    }

    pub fn apply(self, ctx: &egui::Context, light: bool) {
        // Pin the selected theme before installing visuals: otherwise the first
        // Windows system-theme event can switch egui back to its light style.
        ctx.set_theme(if light {
            egui::Theme::Light
        } else {
            egui::Theme::Dark
        });
        let mut visuals = if light {
            egui::Visuals::light()
        } else {
            egui::Visuals::dark()
        };
        visuals.panel_fill = self.panel;
        visuals.window_fill = self.panel;
        visuals.extreme_bg_color = self.bg;
        visuals.faint_bg_color = self.raised;
        visuals.override_text_color = None;
        visuals.weak_text_color = Some(self.muted);
        for widget in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            widget.fg_stroke.color = self.text;
            widget.corner_radius = egui::CornerRadius::same(1);
        }
        visuals.window_corner_radius = egui::CornerRadius::same(2);
        visuals.menu_corner_radius = egui::CornerRadius::same(1);
        visuals.selection.bg_fill = self.accent.gamma_multiply(0.3);
        visuals.selection.stroke = Stroke::new(1.0_f32, self.accent);
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, self.line);
        visuals.widgets.inactive.weak_bg_fill = self.raised;
        visuals.widgets.hovered.weak_bg_fill = self.accent.gamma_multiply(0.15);
        visuals.widgets.active.weak_bg_fill = self.accent.gamma_multiply(0.25);
        ctx.set_visuals(visuals);
        ctx.style_mut(|style| {
            style.spacing.item_spacing = egui::vec2(4.0, 3.0);
            style.spacing.button_padding = egui::vec2(6.0, 3.0);
            style.spacing.interact_size.y = 22.0;
            style
                .text_styles
                .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
            style
                .text_styles
                .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
        });
    }
}

#[cfg(windows)]
struct FontFile {
    // Keep the read-only handle open. On Windows deny other handles write/delete
    // sharing so the borrowed mapping cannot be invalidated by a font update.
    _file: std::fs::File,
    mapping: memmap2::Mmap,
}
#[cfg(windows)]
impl FontFile {
    fn open(path: &str) -> std::io::Result<Self> {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1); // FILE_SHARE_READ
        }
        let file = options.open(path)?;
        // SAFETY: only installed system font files are mapped, read-only. The
        // retained Windows handle excludes write/delete access for the lifetime
        // of the mapping; the map is never mutated and outlives all FontRefs.
        let mapping = unsafe { memmap2::Mmap::map(&file)? };
        Ok(Self {
            _file: file,
            mapping,
        })
    }
    fn bytes(&self) -> &[u8] {
        &self.mapping
    }
}

#[cfg(not(windows))]
struct FontFile(Vec<u8>);
#[cfg(not(windows))]
impl FontFile {
    fn open(path: &str) -> std::io::Result<Self> {
        std::fs::read(path).map(Self)
    }
    fn bytes(&self) -> &[u8] {
        &self.0
    }
}

pub fn load_fonts(ctx: &egui::Context) {
    // File-backed pages are faulted in on demand, rather than reading the whole
    // ~19 MiB CJK collection into a private heap allocation at startup.
    static SYSTEM_FONTS: std::sync::OnceLock<Vec<(&str, FontFile, bool)>> =
        std::sync::OnceLock::new();
    let mut fonts = FontDefinitions::default();
    let system_fonts = SYSTEM_FONTS.get_or_init(|| {
        let candidates = if cfg!(windows) {
            vec![
                ("mono", "C:/Windows/Fonts/consola.ttf", true),
                ("cjk", "C:/Windows/Fonts/msyh.ttc", false),
            ]
        } else if cfg!(target_os = "macos") {
            vec![
                ("mono", "/System/Library/Fonts/Menlo.ttc", true),
                ("cjk", "/System/Library/Fonts/PingFang.ttc", false),
            ]
        } else {
            vec![
                (
                    "mono",
                    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
                    true,
                ),
                (
                    "cjk",
                    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
                    false,
                ),
            ]
        };
        candidates
            .into_iter()
            .filter_map(|(name, path, mono)| {
                FontFile::open(path).ok().map(|bytes| (name, bytes, mono))
            })
            .collect()
    });
    for &(name, ref bytes, mono) in system_fonts {
        fonts
            .font_data
            .insert(name.into(), FontData::from_static(bytes.bytes()).into());
        if mono {
            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .insert(0, name.into());
        } else {
            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .push(name.into());
            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .push(name.into());
        }
    }
    ctx.set_fonts(fonts);
}

pub fn ansi_color(color: vt100::Color, default: Color32, bold: bool) -> Color32 {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
        vt100::Color::Idx(mut index) => {
            if bold && index < 8 {
                index += 8;
            }
            let colors = [
                [35, 43, 56],
                [239, 111, 120],
                [117, 212, 153],
                [235, 202, 125],
                [121, 171, 245],
                [191, 150, 240],
                [103, 205, 218],
                [203, 214, 228],
                [107, 124, 146],
                [255, 143, 152],
                [157, 235, 178],
                [255, 226, 162],
                [157, 198, 255],
                [215, 179, 255],
                [148, 234, 244],
                [245, 247, 250],
            ];
            let [r, g, b] = if index < 16 {
                colors[index as usize]
            } else if index < 232 {
                let n = index - 16;
                let levels = [0, 95, 135, 175, 215, 255];
                [
                    levels[(n / 36) as usize],
                    levels[((n / 6) % 6) as usize],
                    levels[(n % 6) as usize],
                ]
            } else {
                [8 + (index - 232) * 10; 3]
            };
            Color32::from_rgb(r, g, b)
        }
    }
}
