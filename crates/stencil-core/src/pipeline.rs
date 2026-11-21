//! End-to-end processing: source image + palette + settings -> segmentation.

use crate::color::DistanceMetric;
use crate::palette::Palette;
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
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProcessResult {
    pub segmentation: Segmentation,
    /// Palette indices actually chosen for this image (sorted).
    pub selected: Vec<usize>,
    /// Pixel count per palette index (len == palette.len()).
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
    let (w, h) = img.dimensions();
    let smoothed;
    let src = if settings.smoothing > 0 {
        smoothed = imageops::blur(img, settings.smoothing as f32 * 0.8);
        &smoothed
    } else {
        img
    };

    let weights = settings.focus_on_subject.then(|| quantize::detail_weights(src));
    let hist = Histogram::build_weighted(src, weights.as_deref());
    let selected = match selection {
        Some(sel) => sel.to_vec(),
        None => quantize::select_palette_subset(&hist, palette, settings.color_count, settings.metric),
    };
    let colors = quantize::map_pixels(src, &hist, palette, &selected, settings.metric);

    let px_per_mm = settings.output.px_per_mm(w, h);
    let min_region_px = (settings.min_region_mm2 * px_per_mm * px_per_mm).round().max(1.0) as u32;
    let segmentation = Segmentation::from_color_map(w, h, &colors).merge_small_regions(min_region_px);

    let mut usage = vec![0u32; palette.len()];
    for (r, &c) in segmentation.region_color.iter().enumerate() {
        usage[c as usize] += segmentation.region_area[r];
    }
    ProcessResult {
        segmentation,
        selected,
        usage,
        output_mm: settings.output.fitted_mm(w, h),
        min_region_px,
        px_per_mm,
    }
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
}
