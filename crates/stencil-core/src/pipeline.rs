//! End-to-end processing: source image + palette + settings -> segmentation.

use crate::color::{DistanceMetric, Rgb8};
use crate::palette::{Palette, Swatch};
use crate::paper::OutputSize;
use crate::quantize::{self, Histogram};
use crate::regions::Segmentation;
use image::{RgbImage, imageops};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    Stencil,
    PaintByNumbers,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub mode: Mode,
    /// Number of palette colors to use (clamped to the palette size).
    pub color_count: usize,
    /// Regions smaller than this physical area (mm²) are merged into a neighbor.
    pub min_region_mm2: f32,
    /// Pre-smoothing blur radius in pixels (0 = off). Reduces photo noise before quantizing.
    pub smoothing: u32,
    pub metric: DistanceMetric,
    pub output: OutputSize,
    /// Gaussian radius used to smooth vector outlines (mm at the printed size).
    pub curve_smoothing_mm: f32,
    /// Paint-by-numbers: smooth vector outlines (true) or pixel-exact raster outlines.
    pub smooth_outlines: bool,
    /// Weight sharp, detailed areas more when choosing colors, so the in-focus subject
    /// gets its colors ahead of a blurry background.
    #[serde(default = "default_true")]
    pub focus_on_subject: bool,
    /// Cut the subject out: the background becomes one flat area that is never a
    /// stencil layer, shown in `background_color`.
    #[serde(default)]
    pub remove_background: bool,
    #[serde(default = "default_background")]
    pub background_color: Rgb8,
}

fn default_background() -> Rgb8 {
    Rgb8::new(255, 255, 255)
}

fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: Mode::Stencil,
            color_count: 3,
            min_region_mm2: 4.0,
            smoothing: 1,
            metric: DistanceMetric::Ciede2000,
            output: OutputSize::default(),
            curve_smoothing_mm: 0.4,
            smooth_outlines: true,
            focus_on_subject: true,
            remove_background: false,
            background_color: default_background(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProcessResult {
    pub segmentation: Segmentation,
    /// The palette the segmentation indexes into: the input palette, plus a final
    /// "Background" swatch when the background was removed.
    pub palette: Palette,
    /// Palette indices actually chosen for this image (sorted).
    pub selected: Vec<usize>,
    /// Pixel count per palette index (len == `palette.len()` of the result).
    pub usage: Vec<u32>,
    /// Physical size of the fitted output in mm.
    pub output_mm: (f32, f32),
    /// Merge threshold in pixels that was applied.
    pub min_region_px: u32,
    /// Image pixels per millimetre at the printed size.
    pub px_per_mm: f32,
}

impl ProcessResult {
    /// Smoothing radius in image pixels for these settings.
    pub fn sigma_px(&self, settings: &Settings) -> f32 {
        settings.curve_smoothing_mm * self.px_per_mm
    }
}

/// Downscale so the longest side is at most `max_side` (no-op if already smaller).
pub fn fit_within(img: &RgbImage, max_side: u32) -> RgbImage {
    let (w, h) = img.dimensions();
    if w.max(h) <= max_side {
        return img.clone();
    }
    let s = max_side as f32 / w.max(h) as f32;
    let (nw, nh) = (
        ((w as f32 * s).round() as u32).max(1),
        ((h as f32 * s).round() as u32).max(1),
    );
    imageops::resize(img, nw, nh, imageops::FilterType::Triangle)
}

pub fn process(img: &RgbImage, palette: &Palette, settings: &Settings) -> ProcessResult {
    process_with_selection(img, palette, None, settings)
}

/// Like [`process`], but optionally forces which palette colors are used (so a
/// high-resolution export matches the on-screen preview exactly).
pub fn process_with_selection(
    img: &RgbImage,
    palette: &Palette,
    selection: Option<&[usize]>,
    settings: &Settings,
) -> ProcessResult {
    let mask = settings
        .remove_background
        .then(|| crate::background::foreground_mask(img));
    process_masked(img, palette, selection, settings, mask.as_deref())
}

/// Like [`process_with_selection`] with a precomputed subject mask (`true` = subject),
/// used when `settings.remove_background` is on. Lets callers cache the mask.
pub fn process_masked(
    img: &RgbImage,
    palette: &Palette,
    selection: Option<&[usize]>,
    settings: &Settings,
    subject: Option<&[bool]>,
) -> ProcessResult {
    let (w, h) = img.dimensions();
    let subject = subject.filter(|m| settings.remove_background && m.len() == (w * h) as usize);
    let smoothed;
    let src = if settings.smoothing > 0 {
        smoothed = imageops::blur(img, settings.smoothing as f32 * 0.8);
        &smoothed
    } else {
        img
    };

    let weights = subject_weights(src, settings, subject);
    let hist = Histogram::build_weighted(src, weights.as_deref());
    let selected = match selection {
        Some(sel) => sel.to_vec(),
        None => quantize::select_palette_subset(&hist, palette, settings.color_count, settings.metric),
    };
    let mut colors = quantize::map_pixels(src, &hist, palette, &selected, settings.metric);
    let mut palette = palette.clone();
    let mut background = None;
    if let Some(m) = subject {
        let bg = palette.len() as u16;
        palette
            .swatches
            .push(Swatch::new("Background", settings.background_color));
        background = Some(bg);
        for (c, &fg) in colors.iter_mut().zip(m) {
            if !fg {
                *c = bg;
            }
        }
    }

    let px_per_mm = settings.output.px_per_mm(w, h);
    let min_region_px = (settings.min_region_mm2 * px_per_mm * px_per_mm).round().max(1.0) as u32;
    let mut segmentation = Segmentation::from_color_map(w, h, &colors);
    segmentation.background = background;
    let segmentation = segmentation.merge_small_regions(min_region_px);

    let mut usage = vec![0u32; palette.len()];
    for (r, &c) in segmentation.region_color.iter().enumerate() {
        usage[c as usize] += segmentation.region_area[r];
    }
    ProcessResult {
        segmentation,
        palette,
        selected,
        usage,
        output_mm: settings.output.fitted_mm(w, h),
        min_region_px,
        px_per_mm,
    }
}

/// Histogram weights: detail weighting (if on) times the subject mask (background
/// pixels count for nothing, so all colours go to the subject).
pub fn subject_weights(img: &RgbImage, settings: &Settings, subject: Option<&[bool]>) -> Option<Vec<f32>> {
    let mut w = settings.focus_on_subject.then(|| quantize::detail_weights(img));
    if let Some(m) = subject.filter(|m| m.len() == (img.width() * img.height()) as usize && m.contains(&true)) {
        let w = w.get_or_insert_with(|| vec![1.0; m.len()]);
        for (v, &f) in w.iter_mut().zip(m) {
            if !f {
                *v = 0.0;
            }
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_to_end_on_gradient() {
        let img = RgbImage::from_fn(120, 80, |x, y| image::Rgb([(x * 2) as u8, (y * 3) as u8, 128]));
        let pal = Palette::basic_acrylic_12();
        let s = Settings {
            color_count: 4,
            ..Default::default()
        };
        let r = process(&img, &pal, &s);
        assert!(r.selected.len() == 4);
        assert_eq!(r.usage.iter().sum::<u32>(), 120 * 80);
        // every region must meet the threshold unless it is the only region
        if r.segmentation.region_count() > 1 {
            assert!(r.segmentation.region_area.iter().all(|&a| a >= r.min_region_px));
        }
    }

    #[test]
    fn removed_background_is_not_a_layer() {
        let img = RgbImage::from_fn(200, 160, |x, y| {
            let (dx, dy) = (x as f32 - 100.0, y as f32 - 80.0);
            if dx * dx + dy * dy < 45.0 * 45.0 {
                if x < 100 {
                    image::Rgb([220, 30, 30])
                } else {
                    image::Rgb([30, 60, 220])
                }
            } else {
                image::Rgb([90, 160, 90])
            }
        });
        let pal = Palette::basic_acrylic_12();
        let s = Settings {
            color_count: 2,
            remove_background: true,
            background_color: Rgb8::new(0, 0, 0),
            ..Default::default()
        };
        let r = process(&img, &pal, &s);
        assert_eq!(r.palette.len(), pal.len() + 1);
        let bg = r.segmentation.background.unwrap();
        assert_eq!(r.palette.swatches[bg as usize].color, Rgb8::new(0, 0, 0));
        assert!(r.usage[bg as usize] > 200 * 160 / 3);
        let layers = crate::stencil::layer_order(&r.segmentation, &r.palette);
        assert_eq!(layers.len(), 2, "two subject colours, no background layer");
        assert!(!layers.contains(&bg));
    }
}
