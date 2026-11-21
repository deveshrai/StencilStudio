//! Background processing thread. Requests are coalesced so only the newest job
//! runs; results are handed back to the UI thread as pixel buffers.

use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};
use stencil_core::quantize::{Histogram, auto_palette};
use stencil_core::{Palette, Rgb8, Settings, process, render};

pub struct Job {
    pub generation: u64,
    pub image: Arc<image::RgbImage>,
    /// `None` = derive the palette from the image (auto mode).
    pub palette: Option<Palette>,
    pub settings: Settings,
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
                let palette = job
                    .palette
                    .unwrap_or_else(|| auto_palette(&Histogram::build(&job.image), job.settings.color_count));
                let r = process(&job.image, &palette, &job.settings);
                let line = match job.settings.mode {
                    stencil_core::Mode::Stencil => Rgb8::new(20, 20, 20),
                    stencil_core::Mode::PaintByNumbers => Rgb8::new(150, 150, 150),
                };
                let reduced = to_pixel_buffer(&render::render_reduced(&r.segmentation, &palette));
                let outlines = to_pixel_buffer(&render::render_outlines(&r.segmentation, line));
                deliver(JobResult {
                    generation: job.generation,
                    region_count: r.segmentation.region_count(),
                    min_region_px: r.min_region_px,
                    output_mm: r.output_mm,
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
