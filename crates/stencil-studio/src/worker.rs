//! Background processing thread. Requests are coalesced so only the newest job
//! runs; results are handed back to the UI thread as pixel buffers.

use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};
use stencil_core::quantize::{Histogram, auto_palette, detail_weights};
use stencil_core::stencil::{Bridge, StencilOptions, build_layer_for, layer_order};
use stencil_core::vector::SmoothBoundaries;
use stencil_core::{Mode, Palette, Rgb8, Settings, process, render};

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
            while let Ok(mut job) = rx.recv() {
                // Coalesce: skip to the newest pending request.
                while let Ok(newer) = rx.try_recv() {
                    job = newer;
                }
                let t0 = Instant::now();
                let palette = job.palette.unwrap_or_else(|| {
                    let w = job.settings.focus_on_subject.then(|| detail_weights(&job.image));
                    auto_palette(
                        &Histogram::build_weighted(&job.image, w.as_deref()),
                        job.settings.color_count,
                    )
                });
                let r = process(&job.image, &palette, &job.settings);
                let smooth = job.settings.mode == Mode::Stencil || job.settings.smooth_outlines;
                let mut layers = Vec::new();
                let mut layer_view = None;
                let (reduced, outlines) = if smooth {
                    let sb = SmoothBoundaries::build(&r.segmentation, r.sigma_px(&job.settings));
                    if job.settings.mode == Mode::Stencil {
                        let order = layer_order(&r.segmentation, &palette);
                        layers = order
                            .iter()
                            .map(|&c| {
                                (
                                    c,
                                    palette.swatches[c as usize].color,
                                    palette.swatches[c as usize].name.clone(),
                                )
                            })
                            .collect();
                        if let Some(req) = &job.layer {
                            let index = req.index.min(order.len().saturating_sub(1));
                            if let Some(&color) = order.get(index) {
                                let l = build_layer_for(
                                    &r.segmentation,
                                    &sb,
                                    &palette,
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
                    (render::render_smooth_fill(&r.segmentation, &sb, &palette), lines)
                } else {
                    (
                        render::render_reduced(&r.segmentation, &palette),
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
                    selected: r.selected,
                    usage: r.usage,
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
