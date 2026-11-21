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

/// Preview of one stencil sheet, cropped to the image area so it lines up with the
/// other views: cut-outs in the layer's paint color, sheet material dark, bridges
/// highlighted (blue automatic, green manual) and floating pieces in red.
pub fn render_layer_preview(layer: &crate::stencil::StencilLayer, img_w: u32, img_h: u32, px_per_mm: f32) -> RgbaImage {
    let scale = (2400.0 / img_w.max(img_h) as f32).clamp(1.0, 2.0);
    let (w, h) = (
        (img_w as f32 * scale).round() as u32,
        (img_h as f32 * scale).round() as u32,
    );
    let mut pm = tiny_skia::Pixmap::new(w.max(1), h.max(1)).expect("pixmap");
    // Cut-outs show the paint; the sheet material must contrast with it, so dark paints
    // get a light "card" sheet and light paints a dark one.
    let c = layer.color;
    let dark_paint = c.to_lab().l < 50.0;
    let (cut, material) = if dark_paint {
        (c, Rgb8::new(222, 218, 208))
    } else {
        let mix = |v: u8| ((v as u16 * 3 + 255) / 4) as u8;
        (Rgb8::new(mix(c.r), mix(c.g), mix(c.b)), Rgb8::new(58, 61, 70))
    };
    pm.fill(tiny_skia::Color::from_rgba8(cut.r, cut.g, cut.b, 255));
    let k = px_per_mm * scale;
    let off = layer.image_offset_mm;
    let tf = tiny_skia::Transform::from_row(k, 0.0, 0.0, k, -off * k, -off * k);
    let paint = |r: u8, g: u8, b: u8, a: u8| {
        let mut p = tiny_skia::Paint::default();
        p.set_color_rgba8(r, g, b, a);
        p.anti_alias = true;
        p
    };
    for (i, piece) in layer.pieces.iter().enumerate() {
        let rings: Vec<Vec<crate::vector::Pt>> = std::iter::once(piece.outer.clone())
            .chain(piece.holes.iter().cloned())
            .collect();
        if let Some(path) = build_path(&rings, true) {
            let p = if i == 0 {
                paint(material.r, material.g, material.b, 255)
            } else {
                paint(220, 50, 50, 255)
            };
            pm.fill_path(&path, &p, tiny_skia::FillRule::EvenOdd, tf, None);
        }
    }
    for b in &layer.bridges {
        let r = b.rect_mm;
        if let Some(rect) = tiny_skia::Rect::from_ltrb(r[0], r[1], r[2], r[3]) {
            let path = tiny_skia::PathBuilder::from_rect(rect);
            let (fill, edge) = if b.manual {
                (paint(46, 204, 113, 200), paint(20, 120, 60, 255))
            } else {
                (paint(76, 141, 255, 200), paint(30, 80, 170, 255))
            };
            pm.fill_path(&path, &fill, tiny_skia::FillRule::Winding, tf, None);
            let stroke = tiny_skia::Stroke {
                width: 1.5 / k,
                ..Default::default()
            };
            pm.stroke_path(&path, &edge, &stroke, tf, None);
        }
    }
    to_rgba_image(pm)
}
