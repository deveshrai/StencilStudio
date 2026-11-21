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
