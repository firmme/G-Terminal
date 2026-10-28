//! Rasterize shaped runs (including color font glyphs) into bounded cached GPU textures.
use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Wrap};
use eframe::egui::{self, Color32, TextureHandle};
use std::collections::{HashMap, VecDeque};

const MAX_GLYPH_CACHE_ENTRIES: usize = 4096;
const MAX_GLYPH_CACHE_BYTES: usize = 8 * 1024 * 1024;

/// Swash keeps CPU glyph bitmaps independently of our bounded GPU run cache.
/// New glyphs, font sizes and DPI scales must not accumulate for the lifetime
/// of the UI thread. Reset the scaler too so its allocations are released.
fn trim_glyph_cache(swash: &mut SwashCache) {
    if swash.image_cache.len() > MAX_GLYPH_CACHE_ENTRIES
        || swash
            .image_cache
            .values()
            .flatten()
            .map(|image| image.data.len())
            .sum::<usize>()
            > MAX_GLYPH_CACHE_BYTES
    {
        *swash = SwashCache::new();
    }
}

pub struct TextRenderer {
    fonts: FontSystem,
    swash: SwashCache,
    textures: HashMap<String, TextureHandle>,
    order: VecDeque<String>,
}
impl TextRenderer {
    pub fn new() -> Self {
        let mut fonts = FontSystem::new();
        // Match the face the UI loads in `theme`: Consolas on Windows, Menlo on
        // macOS. A generic "monospace" family is not guaranteed to resolve to a
        // real face, and a proportional fallback would break cell alignment.
        fonts.db_mut().set_monospace_family(if cfg!(windows) {
            "Consolas"
        } else if cfg!(target_os = "macos") {
            "Menlo"
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
        trim_glyph_cache(&mut self.swash);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn key(glyph: u16) -> cosmic_text::CacheKey {
        cosmic_text::CacheKey::new(
            cosmic_text::fontdb::ID::default(),
            glyph,
            16.0,
            (0.0, 0.0),
            cosmic_text::fontdb::Weight::NORMAL,
            cosmic_text::CacheKeyFlags::empty(),
        )
        .0
    }

    #[test]
    fn glyph_cache_releases_bitmap_memory_and_negative_entries() {
        let mut cache = SwashCache::new();
        let mut image = cosmic_text::SwashImage::new();
        image.data = vec![0; MAX_GLYPH_CACHE_BYTES];
        cache.image_cache.insert(key(0), Some(image));
        trim_glyph_cache(&mut cache);
        assert_eq!(cache.image_cache.len(), 1, "keep a cache within budget");
        let mut image = cosmic_text::SwashImage::new();
        image.data = vec![0; 1];
        cache.image_cache.insert(key(1), Some(image));
        trim_glyph_cache(&mut cache);
        assert!(
            cache.image_cache.is_empty(),
            "bitmap memory must be released"
        );
        for glyph in 0..=MAX_GLYPH_CACHE_ENTRIES as u16 {
            cache.image_cache.insert(key(glyph), None);
        }
        trim_glyph_cache(&mut cache);
        assert!(
            cache.image_cache.is_empty(),
            "missing glyphs must also be bounded"
        );
    }
}
