// Hide the console window on Windows release builds.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod export_job;
mod print;
mod worker;

use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use stencil_core::pipeline::fit_within;
use stencil_core::{DistanceMetric, Mode, OutputSize, Palette, PaperSize, Rgb8, Settings, Swatch};

slint::include_modules!();

/// Longest side of the image used for live preview processing.
const WORKING_MAX_SIDE: u32 = 1600;
/// Upper bound on colors when the palette is auto-generated from the image.
const AUTO_MAX_COLORS: i32 = 24;

struct State {
    /// Full-resolution source (kept for final exports).
    source: Option<image::RgbImage>,
    /// Downscaled copy used for live preview.
    working: Option<Arc<image::RgbImage>>,
    /// User palettes. Palette source index 0 is "Auto", index i>0 is `library[i-1]`.
    library: Vec<Palette>,
    auto_palette: Palette,
    papers: Vec<PaperSize>,
    requested_generation: u64,
    applied_generation: u64,
    tx: Option<Sender<worker::Job>>,
    /// Palette + chosen colors of the last applied preview (exports reuse them).
    last_palette: Option<Palette>,
    last_selected: Vec<usize>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        source: None,
        working: None,
        library: Palette::builtins(),
        auto_palette: Palette::new("Auto (from image)"),
        papers: PaperSize::presets(),
        requested_generation: 0,
        applied_generation: 0,
        tx: None,
        last_palette: None,
        last_selected: Vec::new(),
    });
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

fn rgb_to_slint(c: Rgb8) -> slint::Color {
    slint::Color::from_rgb_u8(c.r, c.g, c.b)
}

fn current_palette(ui: &AppWindow, st: &State) -> Option<Palette> {
    match ui.get_palette_source() {
        i if i <= 0 => None,
        i => st.library.get(i as usize - 1).cloned(),
    }
}

fn read_settings(ui: &AppWindow, st: &State) -> Settings {
    let paper = st
        .papers
        .get(ui.get_paper_index().max(0) as usize)
        .cloned()
        .unwrap_or_else(|| st.papers[0].clone());
    Settings {
        mode: if ui.get_mode() == 1 {
            Mode::PaintByNumbers
        } else {
            Mode::Stencil
        },
        color_count: ui.get_color_count().max(1) as usize,
        min_region_mm2: ui.get_min_region(),
        smoothing: ui.get_smoothing().round().max(0.0) as u32,
        metric: if ui.get_metric_index() == 1 {
            DistanceMetric::Cie76
        } else {
            DistanceMetric::Ciede2000
        },
        output: OutputSize {
            paper,
            landscape: ui.get_landscape(),
            margin_mm: 10.0,
        },
        curve_smoothing_mm: ui.get_curve_smoothing(),
        smooth_outlines: ui.get_smooth_outlines(),
    }
}

/// Fill the palette panel list. `selected`/`usage` describe the last processing result.
fn show_swatches(ui: &AppWindow, palette: &Palette, selected: &[usize], usage: &[u32]) {
    let total: u64 = usage.iter().map(|&u| u as u64).sum();
    let items: Vec<SwatchItem> = palette
        .swatches
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let used = usage.get(i).copied().unwrap_or(0);
            SwatchItem {
                color: rgb_to_slint(s.color),
                name: s.name.as_str().into(),
                hex: s.color.to_hex().into(),
                chart: stencil_core::chart::nearest_code(s.color).into(),
                active: selected.contains(&i) && used > 0,
                usage: if used > 0 && total > 0 {
                    format!("{:.0}%", used as f64 * 100.0 / total as f64).into()
                } else {
                    SharedString::new()
                },
            }
        })
        .collect();
    ui.set_swatches(ModelRc::new(VecModel::from(items)));
}

fn sync_palette_ui(ui: &AppWindow, st: &State) {
    let mut names: Vec<SharedString> = vec!["Auto (from image)".into()];
    names.extend(st.library.iter().map(|p| SharedString::from(p.name.as_str())));
    ui.set_palette_sources(ModelRc::new(VecModel::from(names)));

    let max = match current_palette(ui, st) {
        Some(p) => (p.len() as i32).max(1),
        None => AUTO_MAX_COLORS,
    };
    ui.set_max_colors(max);
    ui.set_color_count(ui.get_color_count().clamp(1, max));
    ui.set_palette_editable(ui.get_palette_source() > 0);
    match current_palette(ui, st) {
        Some(p) => show_swatches(ui, &p, &[], &[]),
        None => show_swatches(ui, &st.auto_palette, &[], &[]),
    }
}

fn request_processing(ui: &AppWindow) {
    with_state(|st| {
        let (Some(image), Some(tx)) = (st.working.clone(), st.tx.as_ref()) else {
            return;
        };
        st.requested_generation += 1;
        let job = worker::Job {
            generation: st.requested_generation,
            image,
            palette: current_palette(ui, st),
            settings: read_settings(ui, st),
        };
        if tx.send(job).is_ok() {
            ui.set_busy(true);
        }
    });
}

fn apply_result(ui: &AppWindow, r: worker::JobResult) {
    with_state(|st| {
        if r.generation <= st.applied_generation {
            return;
        }
        st.applied_generation = r.generation;
        if r.generation == st.requested_generation {
            ui.set_busy(false);
        }
        st.last_palette = Some(r.palette.clone());
        st.last_selected = r.selected.clone();
        let is_auto = ui.get_palette_source() <= 0;
        if is_auto {
            st.auto_palette = r.palette.clone();
        }
        // Only refresh the list if it still shows the palette this result was computed with.
        let shown = if is_auto {
            Some(r.palette.clone())
        } else {
            current_palette(ui, st)
        };
        if shown.as_ref() == Some(&r.palette) {
            show_swatches(ui, &r.palette, &r.selected, &r.usage);
        }
    });
    ui.set_reduced_image(Image::from_rgba8(r.reduced));
    ui.set_outline_image(Image::from_rgba8(r.outlines));
    ui.set_region_count_text(r.region_count.to_string().into());
    ui.set_threshold_text(format!("{} px", r.min_region_px).into());
    ui.set_output_size_text(format!("{:.0} × {:.0} mm", r.output_mm.0, r.output_mm.1).into());
    ui.set_timing_text(format!("{} ms", r.elapsed.as_millis()).into());
}

/// Decode an image honoring EXIF orientation; transparent pixels are composited onto white.
fn load_image(path: &Path) -> Result<image::RgbImage, String> {
    use image::ImageDecoder;
    let reader = image::ImageReader::open(path).map_err(|e| e.to_string())?;
    let mut decoder = reader
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .into_decoder()
        .map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().map_err(|e| e.to_string())?;
    let mut img = image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    img.apply_orientation(orientation);
    if !img.color().has_alpha() {
        return Ok(img.to_rgb8());
    }
    let rgba = img.to_rgba8();
    Ok(image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let p = rgba.get_pixel(x, y).0;
        let a = p[3] as u32;
        let blend = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
        image::Rgb([blend(p[0]), blend(p[1]), blend(p[2])])
    }))
}

fn open_image_path(ui: &AppWindow, path: &Path) {
    ui.set_status_text(format!("Loading {}…", path.display()).into());
    match load_image(path) {
        Ok(img) => {
            let working = fit_within(&img, WORKING_MAX_SIDE);
            let rgba = image::DynamicImage::ImageRgb8(working.clone()).to_rgba8();
            ui.set_original_image(Image::from_rgba8(worker::to_pixel_buffer(&rgba)));
            ui.set_reduced_image(Image::default());
            ui.set_outline_image(Image::default());
            ui.set_image_width(working.width() as i32);
            ui.set_image_height(working.height() as i32);
            ui.set_source_size_text(format!("{} × {} px", img.width(), img.height()).into());
            ui.set_file_name(
                path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
                    .into(),
            );
            ui.set_has_image(true);
            ui.set_fit_mode(true);
            ui.set_status_text(format!("Opened {}", path.display()).into());
            with_state(|st| {
                st.source = Some(img);
                st.working = Some(Arc::new(working));
            });
            request_processing(ui);
        }
        Err(e) => {
            ui.set_status_text(format!("Could not open image: {e}").into());
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("Open Image")
                .set_description(format!("Could not open {}:\n{e}", path.display()))
                .show();
        }
    }
}

/// Mutate the currently selected library palette (no-op in auto mode).
fn edit_palette(ui: &AppWindow, f: impl FnOnce(&mut Palette)) {
    let idx = ui.get_palette_source();
    if idx <= 0 {
        return;
    }
    with_state(|st| {
        if let Some(p) = st.library.get_mut(idx as usize - 1) {
            f(p);
        }
        sync_palette_ui(ui, st);
    });
    request_processing(ui);
}

#[derive(Clone, Copy)]
enum CalibrationDoc {
    /// Printed fan-deck chart, matched by eye (never scanned).
    ColorChart,
    /// Fill-in sheet that is photographed/scanned and imported.
    ScanSheet,
}

impl CalibrationDoc {
    fn file_name(self) -> &'static str {
        match self {
            Self::ColorChart => "manual-color-chart.pdf",
            Self::ScanSheet => "palette-scan-sheet.pdf",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Self::ColorChart => "manual color chart",
            Self::ScanSheet => "scan sheet",
        }
    }
}

/// Render a calibration document: US Letter if the user works in US sizes, else A4.
fn calibration_pdf(ui: &AppWindow, doc: CalibrationDoc) -> Vec<u8> {
    let us = with_state(|st| {
        st.papers
            .get(ui.get_paper_index().max(0) as usize)
            .is_some_and(|p| p.name.starts_with("US"))
    });
    let paper = if us {
        PaperSize::new("US Letter", 215.9, 279.4)
    } else {
        PaperSize::new("A4", 210.0, 297.0)
    };
    let (name, names) = with_state(|st| match current_palette(ui, st) {
        Some(p) => (
            p.name.clone(),
            p.swatches.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
        ),
        None => (String::new(), Vec::new()),
    });
    match doc {
        CalibrationDoc::ColorChart => stencil_core::chart::chart_pdf(&paper, &name, &names),
        CalibrationDoc::ScanSheet => stencil_core::calibration::sheet_pdf(&paper, &name, &names),
    }
}

fn save_calibration(ui: &AppWindow, doc: CalibrationDoc) {
    let Some(path) = rfd::FileDialog::new()
        .set_title(format!("Save {}", doc.title()))
        .add_filter("PDF", &["pdf"])
        .set_file_name(doc.file_name())
        .save_file()
    else {
        return;
    };
    match std::fs::write(&path, calibration_pdf(ui, doc)) {
        Ok(()) => ui.set_status_text(format!("Saved {} to {}", doc.title(), path.display()).into()),
        Err(e) => ui.set_status_text(format!("Could not save {}: {e}", doc.title()).into()),
    }
}

fn print_calibration(ui: &AppWindow, doc: CalibrationDoc) {
    let path = std::env::temp_dir().join(format!("StencilStudio-{}", doc.file_name()));
    if let Err(e) = std::fs::write(&path, calibration_pdf(ui, doc)) {
        ui.set_status_text(format!("Could not prepare the {}: {e}", doc.title()).into());
        return;
    }
    ui.set_status_text("Opening the print dialog…".into());
    // The OS print flow can block; keep the UI responsive.
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let msg = match print::print_pdf(&path) {
            Ok(()) => format!("Sent the {} to the print dialog", doc.title()),
            Err(e) => format!("Could not print: {e}"),
        };
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_status_text(msg.into());
            }
        });
    });
}

fn start_export(ui: &AppWindow, format: export_job::Format) {
    let prepared = with_state(|st| {
        let source = st.source.clone()?;
        let palette = st.last_palette.clone()?;
        Some((source, palette, st.last_selected.clone(), read_settings(ui, st)))
    });
    let Some((source, palette, selected, settings)) = prepared else {
        ui.set_status_text("Open an image and wait for the preview before exporting".into());
        return;
    };
    let stem = std::path::Path::new(ui.get_file_name().as_str())
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "stencil".into());
    let Some(target) = rfd::FileDialog::new()
        .set_title(format!("Export Stencil Layers ({})", format.label()))
        .add_filter(format.label(), &[format.ext()])
        .set_file_name(format!("{stem}-stencil.{}", format.ext()))
        .save_file()
    else {
        return;
    };
    let thickness = ui
        .get_thickness_names()
        .row_data(ui.get_thickness_index().max(0) as usize)
        .and_then(|s| s.trim_end_matches(" mm").parse::<f32>().ok())
        .unwrap_or(1.0);
    let options = stencil_core::stencil::StencilOptions {
        thickness_mm: thickness,
        frame_mm: ui.get_frame_width(),
        registration_marks: ui.get_registration_marks(),
        bridge_islands: ui.get_bridge_islands(),
        bridge_width_mm: ui.get_bridge_width(),
        min_island_mm2: ui.get_min_island(),
        ..Default::default()
    };
    let job = export_job::ExportJob {
        source,
        palette,
        selected,
        settings,
        options,
        format,
        target,
    };
    ui.set_exporting(true);
    ui.set_status_text(format!("Exporting {} layers…", format.label()).into());
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let t = std::time::Instant::now();
        let result = export_job::run(job);
        let elapsed = t.elapsed();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            ui.set_exporting(false);
            match result {
                Ok(summary) => {
                    let dir = summary
                        .files
                        .first()
                        .and_then(|p| p.parent())
                        .map(|p| p.display().to_string())
                        .unwrap_or_default();
                    let mut msg = format!(
                        "Exported {} {} layer file(s) to {dir} in {:.1} s",
                        summary.files.len(),
                        format.label(),
                        elapsed.as_secs_f32()
                    );
                    if summary.bridged + summary.dropped > 0 {
                        msg += &format!(
                            "  ·  {} floating piece(s) bridged, {} too small and dropped",
                            summary.bridged, summary.dropped
                        );
                    }
                    if !summary.islands.is_empty() {
                        let total: usize = summary.islands.iter().map(|i| i.2).sum();
                        let worst = summary.islands.iter().max_by_key(|i| i.2).unwrap();
                        msg += &format!(
                            "  ·  ⚠ {total} floating piece(s) will fall out (most in layer {} {}); turn on bridging",
                            worst.0, worst.1
                        );
                    }
                    ui.set_status_text(msg.into());
                }
                Err(e) => {
                    ui.set_status_text(format!("Export failed: {e}").into());
                    rfd::MessageDialog::new()
                        .set_level(rfd::MessageLevel::Error)
                        .set_title("Export failed")
                        .set_description(e)
                        .show();
                }
            }
        });
    });
}

const SHORTCUTS: &str = "\
Ctrl+O\tOpen image
Ctrl+Shift+O\tLoad palette
Ctrl+Shift+S\tSave palette
Ctrl+E\tExport stencil layers (SVG)
Ctrl+P\tPrint manual color chart
Ctrl+1 / 2 / 3 / 4\tOriginal / Preview / Outlines / Split
Ctrl+= / Ctrl+-\tZoom in / out (or Ctrl+scroll)
Ctrl+0\tFit to window
Ctrl+Alt+0\tActual pixels
Ctrl+] / Ctrl+[\tMore / fewer colors
Ctrl+Alt+S / P\tStencil / Paint-by-Numbers mode
Ctrl+Alt+L / R\tToggle palette / properties panel
Ctrl+F1\tCollapse ribbon
Drag\tPan the canvas

On macOS, Ctrl means ⌘ Command.";

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;

    // Worker -> UI thread.
    let weak = ui.as_weak();
    let tx = worker::spawn(move |result| {
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak.upgrade() {
                apply_result(&ui, result);
            }
        });
    });

    with_state(|st| {
        st.tx = Some(tx);
        let papers: Vec<SharedString> = st.papers.iter().map(|p| p.name.as_str().into()).collect();
        ui.set_paper_names(ModelRc::new(VecModel::from(papers)));
        ui.set_palette_source(1); // Basic acrylic set
        sync_palette_ui(&ui, st);
    });

    let w = ui.as_weak();
    ui.on_open_image(move || {
        let ui = w.unwrap();
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Open Image")
            .add_filter("Images", &["jpg", "jpeg", "png", "webp"])
            .pick_file()
        {
            open_image_path(&ui, &path);
        }
    });

    let w = ui.as_weak();
    ui.on_settings_changed(move || request_processing(&w.unwrap()));

    let w = ui.as_weak();
    ui.on_palette_source_selected(move |_| {
        let ui = w.unwrap();
        ui.set_selected_swatch(-1);
        with_state(|st| sync_palette_ui(&ui, st));
        request_processing(&ui);
    });

    let w = ui.as_weak();
    ui.on_load_palette(move || {
        let ui = w.unwrap();
        let Some(path) = rfd::FileDialog::new()
            .set_title("Load Palette")
            .add_filter("Palette", &["json"])
            .pick_file()
        else {
            return;
        };
        match Palette::load(&path) {
            Ok(p) => {
                with_state(|st| {
                    st.library.push(p);
                    ui.set_palette_source(st.library.len() as i32);
                    ui.set_selected_swatch(-1);
                    sync_palette_ui(&ui, st);
                });
                ui.set_status_text(format!("Loaded palette {}", path.display()).into());
                request_processing(&ui);
            }
            Err(e) => ui.set_status_text(format!("Could not load palette: {e}").into()),
        }
    });

    let w = ui.as_weak();
    ui.on_save_palette(move || {
        let ui = w.unwrap();
        let palette = with_state(|st| current_palette(&ui, st).unwrap_or_else(|| st.auto_palette.clone()));
        let Some(path) = rfd::FileDialog::new()
            .set_title("Save Palette")
            .add_filter("Palette", &["json"])
            .set_file_name(format!("{}.json", palette.name))
            .save_file()
        else {
            return;
        };
        match palette.save(&path) {
            Ok(()) => ui.set_status_text(format!("Saved palette to {}", path.display()).into()),
            Err(e) => ui.set_status_text(format!("Could not save palette: {e}").into()),
        }
    });

    let w = ui.as_weak();
    ui.on_add_swatch(move || {
        let ui = w.unwrap();
        edit_palette(&ui, |p| {
            p.swatches
                .push(Swatch::new(format!("Color {}", p.len() + 1), Rgb8::new(128, 128, 128)))
        });
        let n = ui.get_swatches().row_count() as i32;
        ui.set_selected_swatch(n - 1);
    });

    let w = ui.as_weak();
    ui.on_remove_swatch(move |i| {
        let ui = w.unwrap();
        if i < 0 {
            return;
        }
        edit_palette(&ui, |p| {
            if (i as usize) < p.len() && p.len() > 1 {
                p.swatches.remove(i as usize);
            }
        });
        ui.set_selected_swatch(-1);
    });

    let w = ui.as_weak();
    ui.on_swatch_edited(move |i, name, hex| {
        let ui = w.unwrap();
        let Some(color) = Rgb8::from_hex(&hex) else {
            ui.set_status_text(format!("“{hex}” is not a valid hex color (use #RRGGBB)").into());
            return;
        };
        edit_palette(&ui, |p| {
            if let Some(s) = p.swatches.get_mut(i as usize) {
                s.name = name.trim().to_string();
                s.color = color;
            }
        });
    });

    let w = ui.as_weak();
    ui.on_keep_auto_palette(move || {
        let ui = w.unwrap();
        with_state(|st| {
            let mut p = st.auto_palette.clone();
            p.name = format!("Custom palette {}", st.library.len() + 1);
            st.library.push(p);
            ui.set_palette_source(st.library.len() as i32);
            sync_palette_ui(&ui, st);
        });
        request_processing(&ui);
    });

    ui.on_quit_app(|| {
        let _ = slint::quit_event_loop();
    });

    ui.on_show_about(|| {
        rfd::MessageDialog::new()
            .set_title("About StencilStudio")
            .set_description(format!(
                "StencilStudio {}\n\nTurn photos into multi-layer stencils and paint-by-numbers templates, matched to the colors you actually own.\n\nMade with Slint (https://slint.dev), used under the Slint Royalty-free License.",
                env!("CARGO_PKG_VERSION")
            ))
            .show();
    });

    ui.on_show_shortcuts(|| {
        rfd::MessageDialog::new()
            .set_title("Keyboard Shortcuts")
            .set_description(SHORTCUTS)
            .show();
    });

    let w = ui.as_weak();
    ui.on_export_layers(move |fmt| {
        let ui = w.unwrap();
        let Some(format) = export_job::Format::parse(&fmt) else {
            return;
        };
        start_export(&ui, format);
    });

    let w = ui.as_weak();
    ui.on_save_calibration_sheet(move || save_calibration(&w.unwrap(), CalibrationDoc::ScanSheet));
    let w = ui.as_weak();
    ui.on_print_calibration_sheet(move || print_calibration(&w.unwrap(), CalibrationDoc::ScanSheet));
    let w = ui.as_weak();
    ui.on_save_color_chart(move || save_calibration(&w.unwrap(), CalibrationDoc::ColorChart));
    let w = ui.as_weak();
    ui.on_print_color_chart(move || print_calibration(&w.unwrap(), CalibrationDoc::ColorChart));

    let w = ui.as_weak();
    ui.on_swatch_chart_code(move |i, code| {
        let ui = w.unwrap();
        let Some(color) = stencil_core::chart::lookup(&code) else {
            ui.set_status_text(format!("“{code}” is not on the color chart (columns A–P, rows 1–13, e.g. D7)").into());
            return;
        };
        let mut name = String::new();
        edit_palette(&ui, |p| {
            if let Some(s) = p.swatches.get_mut(i as usize) {
                s.color = color;
                name = s.name.clone();
            }
        });
        ui.set_status_text(
            format!(
                "{name} set to chart patch {} ({})",
                code.trim().to_uppercase(),
                color.to_hex()
            )
            .into(),
        );
    });

    let w = ui.as_weak();
    ui.on_not_yet(move |what| {
        w.unwrap()
            .set_status_text(format!("{what} is coming in a later milestone").into())
    });

    // Allow `stencil-studio path/to/image.jpg`.
    if let Some(arg) = std::env::args_os().nth(1) {
        open_image_path(&ui, Path::new(&arg));
    }

    ui.run()
}
