//! StencilStudio core engine: perceptual quantization to a user's physical palette,
//! region segmentation, and (later) vectorization and export.
//!
//! This crate is UI-agnostic and must stay free of any windowing/GUI dependencies.

pub mod background;
pub mod calibration;
pub mod chart;
pub mod color;
pub mod export;
mod islands;
pub mod palette;
pub mod paper;
pub mod pbn;
pub mod pdf;
pub mod pdf_image;
pub mod pipeline;
pub mod quantize;
pub mod regions;
pub mod render;
pub mod scan;
pub mod stencil;
pub mod vector;

pub use color::{DistanceMetric, Lab, Rgb8};
pub use palette::{Palette, Swatch};
pub use paper::{OutputSize, PaperSize};
pub use pipeline::{Mode, ProcessResult, Settings, process};
pub use regions::Segmentation;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("image error: {0}")]
    Image(#[from] image::ImageError),
}

pub type Result<T> = std::result::Result<T, Error>;
