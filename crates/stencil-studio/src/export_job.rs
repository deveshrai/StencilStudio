//! High-resolution stencil export, run on its own thread.

use std::path::{Path, PathBuf};
use stencil_core::pipeline::{fit_within, process_masked};
use stencil_core::stencil::{StencilOptions, build_layers};
use stencil_core::vector::SmoothBoundaries;
use stencil_core::{Palette, Settings, export};

/// Longest side used when generating export geometry.
pub const EXPORT_MAX_SIDE: u32 = 3000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Svg,
    Dxf,
    Stl,
}

impl Format {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "svg" => Some(Self::Svg),
            "dxf" => Some(Self::Dxf),
            "stl" => Some(Self::Stl),
            _ => None,
        }
    }
    pub fn ext(self) -> &'static str {
        match self {
            Self::Svg => "svg",
            Self::Dxf => "dxf",
            Self::Stl => "stl",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Svg => "SVG",
            Self::Dxf => "DXF",
            Self::Stl => "STL",
        }
    }
}

pub struct ExportJob {
    pub source: image::RgbImage,
    pub palette: Palette,
    pub selected: Vec<usize>,
    pub settings: Settings,
    pub options: StencilOptions,
    pub format: Format,
    /// The path the user picked; layer files are written next to it as `<stem>-L<n>-<color>.<ext>`.
    pub target: PathBuf,
}

pub struct ExportSummary {
    pub files: Vec<PathBuf>,
    /// Floating pieces left unconnected: (layer number, color name, count)
    pub islands: Vec<(usize, String, usize)>,
    pub bridged: usize,
    pub dropped: usize,
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

pub fn layer_path(target: &Path, number: usize, color: &str, ext: &str) -> PathBuf {
    let stem = target
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "stencil".into());
    let dir = target.parent().map(Path::to_path_buf).unwrap_or_default();
    dir.join(format!("{stem}-L{number}-{}.{ext}", slug(color)))
}

pub fn run(job: ExportJob) -> Result<ExportSummary, String> {
    let img = fit_within(&job.source, EXPORT_MAX_SIDE);
    let mask = job
        .settings
        .remove_background
        .then(|| crate::subject::foreground_mask(&img).0);
    let r = process_masked(&img, &job.palette, Some(&job.selected), &job.settings, mask.as_deref());
    let sb = SmoothBoundaries::build(&r.segmentation, r.sigma_px(&job.settings));
    let layers = build_layers(&r.segmentation, &sb, &r.palette, r.px_per_mm, &job.options);
    let mut summary = ExportSummary {
        files: Vec::new(),
        islands: Vec::new(),
        bridged: 0,
        dropped: 0,
    };
    for layer in &layers {
        let path = layer_path(&job.target, layer.number, &layer.name, job.format.ext());
        let bytes = match job.format {
            Format::Svg => export::layer_svg(layer, &job.options).into_bytes(),
            Format::Dxf => export::layer_dxf(layer, &job.options).into_bytes(),
            Format::Stl => export::layer_stl(layer, &job.options),
        };
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        summary.files.push(path);
        summary.bridged += layer.bridged;
        summary.dropped += layer.dropped;
        if layer.island_count() > 0 {
            summary
                .islands
                .push((layer.number, layer.name.clone(), layer.island_count()));
        }
    }
    Ok(summary)
}

/// Paint-by-numbers packet (4-page PDF) at export resolution.
pub fn pbn_packet(
    source: &image::RgbImage,
    palette: &Palette,
    selected: &[usize],
    settings: &Settings,
    title: &str,
) -> Vec<u8> {
    let img = fit_within(source, EXPORT_MAX_SIDE);
    let mask = settings
        .remove_background
        .then(|| crate::subject::foreground_mask(&img).0);
    let r = process_masked(&img, palette, Some(selected), settings, mask.as_deref());
    // Pixel-exact outlines keep the staircase (sigma 0), matching the preview.
    let sigma = if settings.smooth_outlines { r.sigma_px(settings) } else { 0.0 };
    let sb = SmoothBoundaries::build(&r.segmentation, sigma);
    stencil_core::pbn::packet_pdf(&stencil_core::pbn::PacketInput {
        seg: &r.segmentation,
        sb: &sb,
        palette: &r.palette,
        output: &settings.output,
        title,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_file_names() {
        let p = layer_path(Path::new("/tmp/out/parrot.svg"), 2, "Cadmium Red (Hue)", "svg");
        assert_eq!(p, PathBuf::from("/tmp/out/parrot-L2-cadmium-red-hue.svg"));
    }

    #[test]
    fn exports_every_format_end_to_end() {
        let dir = std::env::temp_dir().join(format!("stencil-export-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // red disc with a white hole on a dark background
        let source = image::RgbImage::from_fn(300, 200, |x, y| {
            let r = ((x as f32 - 150.0).powi(2) + (y as f32 - 100.0).powi(2)).sqrt();
            if r < 30.0 {
                image::Rgb([250, 250, 250])
            } else if r < 80.0 {
                image::Rgb([200, 30, 30])
            } else {
                image::Rgb([20, 20, 20])
            }
        });
        let palette = Palette::basic_acrylic_12();
        let idx = |n: &str| palette.swatches.iter().position(|s| s.name == n).unwrap();
        let selected = vec![idx("Titanium White"), idx("Cadmium Red"), idx("Mars Black")];
        for format in [Format::Svg, Format::Dxf, Format::Stl] {
            let job = ExportJob {
                source: source.clone(),
                palette: palette.clone(),
                selected: selected.clone(),
                settings: Settings {
                    color_count: 3,
                    ..Default::default()
                },
                options: StencilOptions::default(),
                format,
                target: dir.join(format!("disc.{}", format.ext())),
            };
            let summary = run(job).unwrap();
            assert_eq!(summary.files.len(), 3, "{format:?}");
            assert!(summary.files.iter().all(|f| f.metadata().unwrap().len() > 200));
            // Red layer: the white centre would float. Black layer: the whole disc would
            // float inside the black cut. Both are bridged instead.
            assert!(summary.islands.is_empty(), "{:?}", summary.islands);
            assert_eq!((summary.bridged, summary.dropped), (2, 0));
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
