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
    /// Text inputs and other sunk surfaces. Kept clearly apart from `panel` so a
    /// field reads as a field instead of dissolving into the dialog.
    pub field: Color32,
    pub danger: Color32,
    pub warn: Color32,
    /// A live connection. Green for "running" is a status convention, separate
    /// from the file-type colours below.
    pub ok: Color32,
    /// File-type colours, following the `ls` convention so the list agrees with
    /// what the terminal shows for the same entry: blue directories, cyan
    /// symlinks, green executables. They are the terminal's own blue, cyan and
    /// green, so the two views share one colour language.
    pub directory: Color32,
    pub symlink: Color32,
    pub executable: Color32,
}

impl Palette {
    pub fn new(light: bool) -> Self {
        if light {
            Self {
                bg: Color32::from_rgb(247, 249, 251),
                panel: Color32::from_rgb(238, 242, 247),
                raised: Color32::WHITE,
                line: Color32::from_rgb(212, 221, 231),
                text: Color32::from_rgb(32, 44, 63),
                muted: Color32::from_rgb(93, 111, 134),
                accent: Color32::from_rgb(13, 122, 104),
                // On a light panel the readable equivalent of "sunk" is a white
                // field against the grey panel, not a darker one.
                field: Color32::WHITE,
                danger: Color32::from_rgb(217, 48, 54),
                warn: Color32::from_rgb(168, 106, 0),
                ok: Color32::from_rgb(28, 132, 66),
                // Darker than the terminal's versions, which are tuned for a dark
                // background and would wash out here.
                directory: Color32::from_rgb(30, 90, 190),
                symlink: Color32::from_rgb(0, 120, 145),
                executable: Color32::from_rgb(20, 120, 60),
            }
        } else {
            Self {
                bg: Color32::from_rgb(13, 18, 26),
                panel: Color32::from_rgb(21, 29, 40),
                raised: Color32::from_rgb(30, 40, 54),
                line: Color32::from_rgb(43, 58, 75),
                text: Color32::from_rgb(218, 227, 238),
                muted: Color32::from_rgb(138, 155, 176),
                accent: Color32::from_rgb(79, 209, 165),
                field: Color32::from_rgb(11, 16, 23),
                danger: Color32::from_rgb(229, 72, 77),
                warn: Color32::from_rgb(217, 164, 65),
                ok: Color32::from_rgb(78, 190, 110),
                // The terminal's blue, cyan and green (see `ansi_color`), so a file
                // is the same colour in the list as it is in `ls` output.
                directory: Color32::from_rgb(121, 171, 245),
                symlink: Color32::from_rgb(103, 205, 218),
                executable: Color32::from_rgb(117, 212, 153),
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
        visuals.extreme_bg_color = self.field;
        visuals.text_edit_bg_color = Some(self.field);
        visuals.faint_bg_color = self.raised;
        visuals.override_text_color = None;
        visuals.weak_text_color = Some(self.muted);
        visuals.error_fg_color = self.danger;
        visuals.warn_fg_color = self.warn;
        visuals.window_stroke = Stroke::new(1.0_f32, self.line);
        // egui's default shadow (offset 10/20, blur 15, black@96) is far too heavy
        // under a frameless window with its own title bar.
        visuals.window_shadow = egui::epaint::Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: Color32::from_black_alpha(120),
        };
        visuals.popup_shadow = egui::epaint::Shadow {
            offset: [0, 4],
            blur: 12,
            spread: 0,
            color: Color32::from_black_alpha(96),
        };
        for widget in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            widget.fg_stroke.color = self.text;
            widget.corner_radius = egui::CornerRadius::same(4);
        }
        visuals.window_corner_radius = egui::CornerRadius::same(6);
        visuals.menu_corner_radius = egui::CornerRadius::same(4);
        visuals.selection.bg_fill = self.accent.gamma_multiply(0.3);
        visuals.selection.stroke = Stroke::new(1.0_f32, self.accent);
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, self.line);
        // Buttons and combo boxes paint `weak_bg_fill`; leaving it at the egui
        // default is what made them look detached from this palette.
        visuals.widgets.inactive.bg_fill = self.raised;
        visuals.widgets.inactive.weak_bg_fill = self.raised;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, self.line);
        visuals.widgets.hovered.bg_fill = self.accent.gamma_multiply(0.18);
        visuals.widgets.hovered.weak_bg_fill = self.accent.gamma_multiply(0.18);
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, self.accent.gamma_multiply(0.6));
        visuals.widgets.active.bg_fill = self.accent.gamma_multiply(0.28);
        visuals.widgets.active.weak_bg_fill = self.accent.gamma_multiply(0.28);
        visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, self.accent);
        visuals.widgets.open.weak_bg_fill = self.accent.gamma_multiply(0.15);
        ctx.set_visuals(visuals);
        ctx.style_mut(|style| {
            style.spacing.item_spacing = egui::vec2(4.0, 3.0);
            style.spacing.button_padding = egui::vec2(6.0, 3.0);
            style.spacing.interact_size.y = 22.0;
            // Hairline scrollbars. The default is 6pt of chrome for what is only
            // an indicator, and the file tables and transfer queue are narrow.
            style.spacing.scroll.bar_width = 4.0;
            style.spacing.scroll.bar_inner_margin = 0.0;
            style.spacing.scroll.bar_outer_margin = 0.0;
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

/// The first of `paths` that exists on disk, or the first entry when none do.
/// The caller only opens what this returns, and a path that is absent is
/// skipped, so an empty result would silently drop the CJK fallback.
fn first_existing(paths: &[&'static str]) -> &'static str {
    paths
        .iter()
        .copied()
        .find(|path| std::path::Path::new(path).is_file())
        .unwrap_or(paths[0])
}

/// How far to drop the CJK fallback so its baseline lines up with the Latin
/// font's, as a fraction of the font size. The right value depends on the
/// fallback face, so it is measured per platform; 0.25 em is macOS (Hiragino /
/// PingFang against egui's Latin font). Windows and Linux have not been
/// measured, so they are left alone rather than guessed.
#[cfg(target_os = "macos")]
const CJK_BASELINE_NUDGE: f32 = 0.25;
#[cfg(not(target_os = "macos"))]
const CJK_BASELINE_NUDGE: f32 = 0.0;

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
            // macOS moved PingFang into the on-demand asset store, so the
            // historical /System/Library/Fonts path is empty on current
            // releases. A missing CJK face leaves every Chinese label as tofu,
            // so the known locations are probed instead of assumed.
            vec![
                ("mono", "/System/Library/Fonts/Menlo.ttc", true),
                (
                    "cjk",
                    first_existing(&[
                        "/System/Library/Fonts/PingFang.ttc",
                        "/System/Library/Fonts/Hiragino Sans GB.ttc",
                        "/System/Library/Fonts/STHeiti Medium.ttc",
                        "/System/Library/Fonts/Supplemental/Songti.ttc",
                    ]),
                    false,
                ),
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
        let mut data = FontData::from_static(bytes.bytes());
        if !mono {
            // The CJK fallback's baseline sits well above the Latin font's
            // (5pt at 20pt on macOS), which leaves every Chinese glyph riding
            // high beside English and digits. Nudge it down onto the Latin
            // baseline; the offset scales with the font size, so one factor
            // covers every text size.
            data.tweak.y_offset_factor = CJK_BASELINE_NUDGE;
        }
        fonts.font_data.insert(name.into(), data.into());
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The file list leans on these to say what a row is. Two of them looking
    /// alike would be worse than no colour at all — the row would claim to be
    /// something it is not. Directories used to be the accent green, which reads
    /// as "executable" to anyone who has used `ls --color`.
    #[test]
    fn file_type_colours_are_mutually_distinguishable() {
        for light in [false, true] {
            let p = Palette::new(light);
            let kinds = [
                ("directory", p.directory),
                ("symlink", p.symlink),
                ("executable", p.executable),
                ("regular", p.text),
            ];
            for (i, (name_a, a)) in kinds.iter().enumerate() {
                for (name_b, b) in &kinds[i + 1..] {
                    let gap: i32 = (0..3)
                        .map(|channel| {
                            (i32::from(a.to_array()[channel]) - i32::from(b.to_array()[channel]))
                                .abs()
                        })
                        .sum();
                    assert!(
                        gap >= 60,
                        "{name_a} and {name_b} are only {gap} apart (light = {light}): {a:?} vs {b:?}"
                    );
                }
            }
        }
    }

    /// A directory is blue and an executable is green, matching `ls --color` and
    /// the terminal's own palette. Getting these the other way round is exactly
    /// the confusion this set exists to remove.
    #[test]
    fn directories_are_blue_and_executables_are_green() {
        for light in [false, true] {
            let p = Palette::new(light);
            let blue = p.directory.to_array();
            assert!(
                blue[2] > blue[0] && blue[2] > blue[1],
                "directory colour is not blue: {blue:?}"
            );
            let green = p.executable.to_array();
            assert!(
                green[1] > green[0] && green[1] > green[2],
                "executable colour is not green: {green:?}"
            );
            let cyan = p.symlink.to_array();
            assert!(
                cyan[1] > cyan[0] && cyan[2] > cyan[0],
                "symlink colour is not cyan: {cyan:?}"
            );
        }
    }

    /// The whole point of the separate `field` colour: an input has to be tellable
    /// from the panel behind it. Fields used to fall back to `extreme_bg_color`,
    /// which was the terminal background — a sum-of-channels gap of 24 against the
    /// panel, which is what made them dissolve into the dialog.
    #[test]
    fn fields_are_distinguishable_from_the_panel() {
        for light in [false, true] {
            let p = Palette::new(light);
            let gap: i32 = (0..3)
                .map(|i| {
                    (i32::from(p.field.to_array()[i]) - i32::from(p.panel.to_array()[i])).abs()
                })
                .sum();
            assert!(
                gap >= 32,
                "field {:?} is too close to panel {:?} (light = {light})",
                p.field,
                p.panel
            );
        }
    }

    /// Surface layers have to be ordered, or a hover reads as a hole.
    #[test]
    fn surfaces_step_monotonically() {
        for light in [false, true] {
            let p = Palette::new(light);
            let luma = |c: Color32| {
                let [r, g, b, _] = c.to_array();
                0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)
            };
            let (bg, panel, raised) = (luma(p.bg), luma(p.panel), luma(p.raised));
            if light {
                assert!(
                    bg > panel && raised > panel,
                    "light layers: {bg} {panel} {raised}"
                );
            } else {
                assert!(panel > bg, "panel must sit above bg: {panel} vs {bg}");
                assert!(
                    raised > panel,
                    "raised must sit above panel: {raised} vs {panel}"
                );
            }
        }
    }
}
