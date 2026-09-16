//! The application mark, as plain pixels.
//!
//! Shared between the running app (which hands it to the window and the taskbar)
//! and the build script (which embeds it in the executable as a file icon), so
//! there is one definition of the mark rather than two that can drift apart.
//!
//! Deliberately dependency-free — no `egui`, no `crate::` — because a build
//! script cannot see any of that.

/// The mark's colours. Fixed rather than theme-derived: a taskbar or Explorer
/// icon is a brand mark, and flipping it with the palette would make it
/// unrecognisable.
pub const ACCENT: [u8; 3] = [79, 209, 165];
pub const SURFACE: [u8; 3] = [21, 29, 40];

/// RGBA pixels of the mark at `size`×`size`, top row first.
pub fn pixels(size: u32) -> Vec<u8> {
    let mut rgba = vec![0u8; (size as usize * size as usize * 4).max(4)];
    let s = size as f32;
    let inset = s * 0.06;
    let radius = s * 0.24;
    let chevron = (s * 0.075).max(1.0);
    // The prompt sits slightly above centre, the way a shell draws it.
    let tip = (s * 0.62, s * 0.5);
    let upper = (s * 0.32, s * 0.33);
    let lower = (s * 0.32, s * 0.67);
    let bar = ((s * 0.34, s * 0.67), (s * 0.68, s * 0.67));

    for y in 0..size {
        for x in 0..size {
            let point = (x as f32 + 0.5, y as f32 + 0.5);
            let inside = (inset..s - inset).contains(&point.0)
                && (inset..s - inset).contains(&point.1)
                && rounded_hit(point, inset, s - inset, radius);
            let pixel = if !inside {
                [0, 0, 0, 0]
            } else if segment_hit(point, upper, tip, chevron)
                || segment_hit(point, tip, lower, chevron)
                || segment_hit(point, bar.0, bar.1, chevron)
            {
                [ACCENT[0], ACCENT[1], ACCENT[2], 255]
            } else {
                [SURFACE[0], SURFACE[1], SURFACE[2], 255]
            };
            let index = ((y as usize * size as usize) + x as usize) * 4;
            rgba[index..index + 4].copy_from_slice(&pixel);
        }
    }
    rgba
}

/// Whether a point falls inside a square with rounded corners.
fn rounded_hit(point: (f32, f32), min: f32, max: f32, radius: f32) -> bool {
    let clamp = |value: f32| value.clamp(min + radius, max - radius);
    let dx = point.0 - clamp(point.0);
    let dy = point.1 - clamp(point.1);
    (dx * dx + dy * dy).sqrt() <= radius
}

/// Distance from a point to a segment, used to rasterise the mark's strokes.
fn segment_hit(point: (f32, f32), from: (f32, f32), to: (f32, f32), width: f32) -> bool {
    let along = (to.0 - from.0, to.1 - from.1);
    let length = along.0 * along.0 + along.1 * along.1;
    let t = if length <= f32::EPSILON {
        0.0
    } else {
        (((point.0 - from.0) * along.0 + (point.1 - from.1) * along.1) / length).clamp(0.0, 1.0)
    };
    let dx = point.0 - (from.0 + along.0 * t);
    let dy = point.1 - (from.1 + along.1 * t);
    (dx * dx + dy * dy).sqrt() <= width * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mark_is_opaque_and_carries_both_colours() {
        let icon = pixels(32);
        assert_eq!(icon.len(), 32 * 32 * 4);
        let opaque = icon.chunks(4).filter(|p| p[3] == 255).count();
        assert!(opaque > 32 * 32 / 2, "only {opaque} pixels are opaque");
        let accent = icon.chunks(4).filter(|p| p[..3] == ACCENT).count();
        let surface = icon.chunks(4).filter(|p| p[..3] == SURFACE).count();
        assert!(accent > 0, "the prompt is missing");
        assert!(surface > 0, "the body is missing");
    }

    #[test]
    fn rounded_square_excludes_its_corners() {
        assert!(rounded_hit((16.0, 16.0), 0.0, 32.0, 8.0));
        assert!(rounded_hit((1.0, 16.0), 0.0, 32.0, 8.0));
        // The very corner is outside the rounded square.
        assert!(!rounded_hit((0.2, 0.2), 0.0, 32.0, 8.0));
    }

    #[test]
    fn segment_distance_covers_the_ends_and_the_middle() {
        let (a, b) = ((0.0, 0.0), (10.0, 0.0));
        assert!(segment_hit((5.0, 0.0), a, b, 2.0));
        assert!(segment_hit((0.0, 0.0), a, b, 2.0));
        assert!(segment_hit((10.0, 0.5), a, b, 2.0));
        assert!(!segment_hit((5.0, 3.0), a, b, 2.0));
        // Beyond either end, the nearest point is the endpoint itself.
        assert!(!segment_hit((-3.0, 0.0), a, b, 2.0));
    }
}
