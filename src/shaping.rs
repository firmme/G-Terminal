//! Rasterize shaped runs (including color font glyphs) into bounded cached GPU textures.
use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Wrap};
use eframe::egui::{self, Color32, TextureHandle};
use std::collections::{HashMap, VecDeque};

pub struct TextRenderer {
    fonts: FontSystem,
    swash: SwashCache,
    textures: HashMap<String, TextureHandle>,
    order: VecDeque<String>,
}
impl TextRenderer {
    pub fn new() -> Self {
        let mut fonts = FontSystem::new();
        fonts.db_mut().set_monospace_family(if cfg!(windows) {
            "Consolas"
        } else {
            "monospace"
        });
        Self {
            fonts,
            swash: SwashCache::new(),
            textures: HashMap::new(),
            order: VecDeque::new(),
        }
    }
    pub fn texture(
        &mut self,
        ctx: &egui::Context,
        text: &str,
        size: f32,
        width: f32,
        height: f32,
        color: Color32,
    ) -> TextureHandle {
        let scale = ctx.pixels_per_point();
        let key = format!("{size}:{width}:{height}:{scale}:{color:?}:{text}");
        if let Some(t) = self.textures.get(&key) {
            return t.clone();
        }
        let w = (width * scale).ceil().clamp(1.0, 8192.0) as usize;
        let h = (height * scale).ceil().clamp(1.0, 256.0) as usize;
        let mut buffer = Buffer::new(&mut self.fonts, Metrics::new(size * scale, height * scale));
        buffer.set_size(&mut self.fonts, Some(w as f32), Some(h as f32));
        buffer.set_wrap(&mut self.fonts, Wrap::None);
        buffer.set_text(
            &mut self.fonts,
            text,
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.fonts, false);
        let mut pixels = vec![0u8; w * h * 4];
        buffer.draw(
            &mut self.fonts,
            &mut self.swash,
            Color::rgba(color.r(), color.g(), color.b(), color.a()),
            |x, y, rw, rh, c| {
                for py in y.max(0)..(y + rh as i32).min(h as i32) {
                    for px in x.max(0)..(x + rw as i32).min(w as i32) {
                        let at = (py as usize * w + px as usize) * 4;
                        let a = c.a() as f32 / 255.0;
                        let old = pixels[at + 3] as f32 / 255.0;
                        let combined = a + old * (1.0 - a);
                        if combined > 0.0 {
                            for (i, component) in [c.r(), c.g(), c.b()].iter().enumerate() {
                                pixels[at + i] = ((*component as f32 * a
                                    + pixels[at + i] as f32 * old * (1.0 - a))
                                    / combined)
                                    as u8;
                            }
                            pixels[at + 3] = (combined * 255.0) as u8;
                        }
                    }
                }
            },
        );
        let texture = ctx.load_texture(
            "shaped-run",
            egui::ColorImage::from_rgba_unmultiplied([w, h], &pixels),
            egui::TextureOptions::LINEAR,
        );
        while self.order.len() >= 256
            || self
                .textures
                .values()
                .map(TextureHandle::byte_size)
                .sum::<usize>()
                + w * h * 4
                > 16 * 1024 * 1024
        {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            self.textures.remove(&old);
        }
        self.order.push_back(key.clone());
        self.textures.insert(key, texture.clone());
        texture
    }
}

thread_local! { static RENDERER: std::cell::RefCell<Option<TextRenderer>> = const { std::cell::RefCell::new(None) }; }
pub fn draw(painter: &egui::Painter, text: &str, rect: egui::Rect, size: f32, color: Color32) {
    RENDERER.with(|cell| {
        let mut renderer = cell.borrow_mut();
        let renderer = renderer.get_or_insert_with(TextRenderer::new);
        let texture = renderer.texture(
            painter.ctx(),
            text,
            size,
            rect.width(),
            rect.height(),
            color,
        );
        painter.image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    });
}
pub fn needs_shaping(text: &str) -> bool {
    text.chars().any(
        |c| matches!(c as u32,0x0300..=0x036f|0x0590..=0x109f|0x1780..=0x17ff|0x1f000..=0x1faff),
    )
}
