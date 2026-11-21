//! Physical output sizes (paper, canvas, laser/3D-printer beds).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaperSize {
    pub name: String,
    /// Width in millimetres (portrait orientation).
    pub width_mm: f32,
    /// Height in millimetres (portrait orientation).
    pub height_mm: f32,
}

impl PaperSize {
    pub fn new(name: &str, width_mm: f32, height_mm: f32) -> Self {
        Self {
            name: name.into(),
            width_mm,
            height_mm,
        }
    }

    pub fn presets() -> Vec<PaperSize> {
        vec![
            Self::new("A4 (210 × 297 mm)", 210.0, 297.0),
            Self::new("A3 (297 × 420 mm)", 297.0, 420.0),
            Self::new("A5 (148 × 210 mm)", 148.0, 210.0),
            Self::new("US Letter (8.5 × 11 in)", 215.9, 279.4),
            Self::new("US Tabloid (11 × 17 in)", 279.4, 431.8),
            Self::new("Canvas 30 × 30 cm", 300.0, 300.0),
            Self::new("Canvas 40 × 50 cm", 400.0, 500.0),
            Self::new("3D bed 200 × 200 mm", 200.0, 200.0),
            Self::new("3D bed 256 × 256 mm", 256.0, 256.0),
            Self::new("Laser bed 300 × 200 mm", 300.0, 200.0),
        ]
    }
}

/// The physical area the image will occupy once printed/cut.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputSize {
    pub paper: PaperSize,
    pub landscape: bool,
    pub margin_mm: f32,
}

impl Default for OutputSize {
    fn default() -> Self {
        Self {
            paper: PaperSize::presets().remove(0),
            landscape: false,
            margin_mm: 10.0,
        }
    }
}

impl OutputSize {
    /// Printable (width, height) in mm after orientation and margins.
    pub fn printable_mm(&self) -> (f32, f32) {
        let (w, h) = if self.landscape {
            (self.paper.height_mm, self.paper.width_mm)
        } else {
            (self.paper.width_mm, self.paper.height_mm)
        };
        ((w - 2.0 * self.margin_mm).max(1.0), (h - 2.0 * self.margin_mm).max(1.0))
    }

    /// Size in mm of the image when fitted (aspect preserved) into the printable area.
    pub fn fitted_mm(&self, img_w: u32, img_h: u32) -> (f32, f32) {
        let (pw, ph) = self.printable_mm();
        let s = (pw / img_w as f32).min(ph / img_h as f32);
        (img_w as f32 * s, img_h as f32 * s)
    }

    /// Image pixels per physical millimetre at the fitted size.
    pub fn px_per_mm(&self, img_w: u32, img_h: u32) -> f32 {
        img_w as f32 / self.fitted_mm(img_w, img_h).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_landscape_image_on_portrait_a4() {
        let o = OutputSize::default(); // A4 portrait, 10 mm margins -> 190 x 277
        let (w, h) = o.fitted_mm(2000, 1000);
        assert!((w - 190.0).abs() < 1e-3 && (h - 95.0).abs() < 1e-3);
        assert!((o.px_per_mm(2000, 1000) - 2000.0 / 190.0).abs() < 1e-3);
    }
}
