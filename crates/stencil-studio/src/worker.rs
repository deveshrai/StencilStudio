//! Background processing thread. Requests are coalesced so only the newest job
//! runs; results are handed back to the UI thread as pixel buffers.

use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};
use stencil_core::pipeline::{process_masked, subject_weights};
use stencil_core::quantize::{Histogram, auto_palette};
use stencil_core::stencil::{Bridge, StencilOptions, build_layer_for, layer_order};
use stencil_core::vector::SmoothBoundaries;
use stencil_core::{Mode, Palette, Rgb8, Settings, background, render};

pub struct Job {
    pub generation: u64,
    pub image: Arc<image::RgbImage>,
    /// `None` = derive the palette from the image (auto mode).
    pub palette: Option<Palette>,
    pub settings: Settings,
    /// Also build this stencil layer (index in painting order) for the Layers view.
    pub layer: Option<LayerRequest>,
}

pub struct LayerRequest {
    pub index: usize,
    pub options: StencilOptions,
}

/// One stencil layer as shown in the Layers view.
pub struct LayerView {
    pub index: usize,
    pub palette_index: u16,
    pub name: String,
    pub image: SharedPixelBuffer<Rgba8Pixel>,
    pub bridges: Vec<Bridge>,
    pub image_offset_mm: f32,
    pub floating: usize,
    pub dropped: usize,
}

pub struct JobResult {
    pub generation: u64,
    pub palette: Palette,
    pub selected: Vec<usize>,
    pub usage: Vec<u32>,
    pub reduced: SharedPixelBuffer<Rgba8Pixel>,
    pub outlines: SharedPixelBuffer<Rgba8Pixel>,
    pub region_count: usize,
    pub min_region_px: u32,
    pub output_mm: (f32, f32),
    pub px_per_mm: f32,
    /// Stencil layers in painting order: (palette index, color, name).
    pub layers: Vec<(u16, Rgb8, String)>,
    pub layer: Option<LayerView>,
    /// With the background removed: its share of the picture, and suggested colours.
    pub background_share: Option<f32>,
    pub recommended: Vec<Rgb8>,
    /// Background colour this result was rendered with.
    pub background_color: Rgb8,
    pub elapsed: Duration,
}

pub fn to_pixel_buffer(img: &image::RgbaImage) -> SharedPixelBuffer<Rgba8Pixel> {
    SharedPixelBuffer::clone_from_slice(img.as_raw(), img.width(), img.height())
}

/// Spawn the worker. `deliver` is called on the worker thread with each result.
pub fn spawn(deliver: impl Fn(JobResult) + Send + 'static) -> Sender<Job> {
    let (tx, rx): (Sender<Job>, Receiver<Job>) = channel();
    std::thread::Builder::new()
        .name("stencil-worker".into())
        .spawn(move || {
            // The subject mask only depends on the image, so keep it while sliders move.
            let mut mask_cache: Option<(Arc<image::RgbImage>, Arc<Vec<bool>>)> = None;
            while let Ok(mut job) = rx.recv() {
                // Coalesce: skip to the newest pending request.
                while let Ok(newer) = rx.try_recv() {
                    job = newer;
                }
                let t0 = Instant::now();
                let mask = job.settings.remove_background.then(|| match &mask_cache {
                    Some((img, m)) if Arc::ptr_eq(img, &job.image) => m.clone(),
                    _ => {
                        let m = Arc::new(background::foreground_mask(&job.image));
                        mask_cache = Some((job.image.clone(), m.clone()));
                        m
                    }
                });
                let mask = mask.as_deref().map(|m| m.as_slice());
                let palette = job.palette.unwrap_or_else(|| {
                    let w = subject_weights(&job.image, &job.settings, mask);
                    auto_palette(
                        &Histogram::build_weighted(&job.image, w.as_deref()),
                        job.settings.color_count,
                    )
                });
                let r = process_masked(&job.image, &palette, None, &job.settings, mask);
                // `full` = palette plus the background swatch (if any); the UI lists `palette`.
                let full = &r.palette;
                let bg = r.segmentation.background;
                let total: u32 = r.usage.iter().sum();
                let subject_px = total - bg.map(|b| r.usage[b as usize]).unwrap_or(0);
                let background_share = bg.map(|b| r.usage[b as usize] as f32 / total.max(1) as f32);
                let recommended = if bg.is_some() {
                    let subject: Vec<(Rgb8, f32)> = (0..palette.len())
                        .filter(|&i| r.usage[i] > 0)
                        .map(|i| (palette.swatches[i].color, r.usage[i] as f32 / subject_px.max(1) as f32))
                        .collect();
                    background::recommend_backgrounds(&subject, &palette, 3)
                } else {
                    Vec::new()
                };
                let mut usage = r.usage.clone();
                usage.truncate(palette.len());
                let smooth = job.settings.mode == Mode::Stencil || job.settings.smooth_outlines;
                let mut layers = Vec::new();
                let mut layer_view = None;
                let (reduced, outlines) = if smooth {
                    let sb = SmoothBoundaries::build(&r.segmentation, r.sigma_px(&job.settings));
                    if job.settings.mode == Mode::Stencil {
                        let order = layer_order(&r.segmentation, full);
                        layers = order
                            .iter()
                            .map(|&c| {
                                (
                                    c,
                                    full.swatches[c as usize].color,
                                    full.swatches[c as usize].name.clone(),
                                )
                            })
                            .collect();
                        if let Some(req) = &job.layer {
                            let index = req.index.min(order.len().saturating_sub(1));
                            if let Some(&color) = order.get(index) {
                                let l = build_layer_for(
                                    &r.segmentation,
                                    &sb,
                                    full,
                                    color,
                                    index + 1,
                                    r.px_per_mm,
                                    &req.options,
                                );
                                let img = render::render_layer_preview(
                                    &l,
                                    r.segmentation.width,
                                    r.segmentation.height,
                                    r.px_per_mm,
                                );
                                layer_view = Some(LayerView {
                                    index,
                                    palette_index: color,
                                    name: l.name.clone(),
                                    image: to_pixel_buffer(&img),
                                    floating: l.island_count(),
                                    dropped: l.dropped,
                                    bridges: l.bridges,
                                    image_offset_mm: l.image_offset_mm,
                                });
                            }
                        }
                    }
                    let line = Rgb8::new(25, 25, 25);
                    let lines = match job.settings.mode {
                        Mode::Stencil => render::render_smooth_outlines(&r.segmentation, &sb, line, 1.3),
                        Mode::PaintByNumbers => {
                            render::render_smooth_outlines(&r.segmentation, &sb, Rgb8::new(120, 120, 120), 1.0)
                        }
                    };
                    (render::render_smooth_fill(&r.segmentation, &sb, full), lines)
                } else {
                    (
                        render::render_reduced(&r.segmentation, full),
                        render::render_outlines(&r.segmentation, Rgb8::new(150, 150, 150)),
                    )
                };
                let reduced = to_pixel_buffer(&reduced);
                let outlines = to_pixel_buffer(&outlines);
                deliver(JobResult {
                    generation: job.generation,
                    region_count: r.segmentation.region_count(),
                    min_region_px: r.min_region_px,
                    output_mm: r.output_mm,
                    px_per_mm: r.px_per_mm,
                    layers,
                    layer: layer_view,
                    selected: r.selected.clone(),
                    usage,
                    background_share,
                    recommended,
                    background_color: job.settings.background_color,
                    palette,
                    reduced,
                    outlines,
                    elapsed: t0.elapsed(),
                });
            }
        })
        .expect("failed to spawn worker thread");
    tx
}
