//! Rasterize processing results for on-screen preview.

use crate::color::Rgb8;
use crate::palette::Palette;
use crate::regions::Segmentation;
use image::{Rgba, RgbaImage};

/// The image as it will look painted with the user's physical colors.
pub fn render_reduced(seg: &Segmentation, palette: &Palette) -> RgbaImage {
    let lut: Vec<Rgb8> = palette.swatches.iter().map(|s| s.color).collect();
    let mut out = RgbaImage::new(seg.width, seg.height);
    for (px, &l) in out.pixels_mut().zip(&seg.labels) {
        let c = lut[seg.region_color[l as usize] as usize];
        *px = Rgba([c.r, c.g, c.b, 255]);
    }
    out
}

/// Region outlines on white (cut lines / paint-by-numbers line art).
pub fn render_outlines(seg: &Segmentation, line: Rgb8) -> RgbaImage {
    RgbaImage::from_fn(seg.width, seg.height, |x, y| {
        if seg.is_boundary(x, y) {
            Rgba([line.r, line.g, line.b, 255])
        } else {
            Rgba([255, 255, 255, 255])
        }
    })
}

/// Scale factor for vector previews so they stay crisp when zoomed (output ≤ ~2400 px).
pub fn vector_preview_scale(seg: &Segmentation) -> f32 {
    (2400.0 / seg.width.max(seg.height) as f32).clamp(1.0, 2.0)
}

fn to_rgba_image(pm: tiny_skia::Pixmap) -> RgbaImage {
    let (w, h) = (pm.width(), pm.height());
    RgbaImage::from_raw(w, h, pm.take_demultiplied()).expect("pixmap size")
}

fn build_path(loops: &[Vec<crate::vector::Pt>], closed: bool) -> Option<tiny_skia::Path> {
    let mut pb = tiny_skia::PathBuilder::new();
    for l in loops {
        let Some(first) = l.first() else { continue };
        pb.move_to(first[0], first[1]);
        for p in &l[1..] {
            pb.line_to(p[0], p[1]);
        }
        if closed {
            pb.close();
        }
    }
    pb.finish()
}

/// Anti-aliased, smoothly filled preview of how the painted result will look.
pub fn render_smooth_fill(seg: &Segmentation, sb: &crate::vector::SmoothBoundaries, palette: &Palette) -> RgbaImage {
    let scale = vector_preview_scale(seg);
    let (w, h) = (
        (seg.width as f32 * scale).round() as u32,
        (seg.height as f32 * scale).round() as u32,
    );
    let mut pm = tiny_skia::Pixmap::new(w.max(1), h.max(1)).expect("pixmap");

    // Area per color; the largest one becomes the background.
    let mut area = vec![0u64; palette.len()];
    for (r, &c) in seg.region_color.iter().enumerate() {
        area[c as usize] += seg.region_area[r] as u64;
    }
    let mut colors: Vec<u16> = (0..palette.len() as u16).filter(|&c| area[c as usize] > 0).collect();
    colors.sort_by_key(|&c| std::cmp::Reverse(area[c as usize]));
    let paint_of = |c: u16| {
        let col = palette.swatches[c as usize].color;
        let mut p = tiny_skia::Paint::default();
        p.set_color_rgba8(col.r, col.g, col.b, 255);
        p.anti_alias = true;
        p
    };
    if let Some(&bg) = colors.first() {
        let col = palette.swatches[bg as usize].color;
        pm.fill(tiny_skia::Color::from_rgba8(col.r, col.g, col.b, 255));
    }
    let tf = tiny_skia::Transform::from_scale(scale, scale);
    // A thin same-color stroke hides anti-aliasing seams between neighbours.
    let seam = tiny_skia::Stroke {
        width: 0.6 / scale,
        ..Default::default()
    };
    for &c in colors.iter().skip(1) {
        let loops = crate::vector::color_contours(seg, sb, c, 0.15);
        if let Some(path) = build_path(&loops, true) {
            let paint = paint_of(c);
            pm.fill_path(&path, &paint, tiny_skia::FillRule::EvenOdd, tf, None);
            pm.stroke_path(&path, &paint, &seam, tf, None);
        }
    }
    to_rgba_image(pm)
}

/// Anti-aliased smooth outlines on white.
pub fn render_smooth_outlines(
    seg: &Segmentation,
    sb: &crate::vector::SmoothBoundaries,
    line: Rgb8,
    width_px: f32,
) -> RgbaImage {
    let scale = vector_preview_scale(seg);
    let (w, h) = (
        (seg.width as f32 * scale).round() as u32,
        (seg.height as f32 * scale).round() as u32,
    );
    let mut pm = tiny_skia::Pixmap::new(w.max(1), h.max(1)).expect("pixmap");
    pm.fill(tiny_skia::Color::WHITE);
    let chains: Vec<Vec<crate::vector::Pt>> = sb
        .chains
        .iter()
        .map(|c| crate::vector::simplify_open(c, 0.15))
        .collect();
    if let Some(path) = build_path(&chains, false) {
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(line.r, line.g, line.b, 255);
        paint.anti_alias = true;
        let stroke = tiny_skia::Stroke {
            width: width_px / scale,
            line_join: tiny_skia::LineJoin::Round,
            line_cap: tiny_skia::LineCap::Round,
            ..Default::default()
        };
        pm.stroke_path(
            &path,
            &paint,
            &stroke,
            tiny_skia::Transform::from_scale(scale, scale),
            None,
        );
    }
    to_rgba_image(pm)
}
