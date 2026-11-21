//! StencilStudio application: shared by the desktop binary (`main.rs`) and the
//! Android `cdylib` (`android_main`).

mod export_job;
mod platform;
#[cfg(not(target_os = "android"))]
mod print;
mod worker;

use platform::PickKind;

use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Sender;
use stencil_core::pipeline::fit_within;
use stencil_core::stencil::{BridgeEdits, StencilOptions};
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
    /// In-app file browser (platforms without native dialogs).
    picker_kind: Option<PickKind>,
    picker_paths: Vec<PathBuf>,
    /// Manual bridge edits per stencil layer (palette index), plus undo history.
    bridge_edits: BTreeMap<u16, BridgeEdits>,
    undo: Vec<BTreeMap<u16, BridgeEdits>>,
    /// The layer currently shown in the Layers view, for click hit-testing.
    layer_hit: Option<LayerHit>,
}

struct LayerHit {
    palette_index: u16,
    bridges: Vec<stencil_core::stencil::Bridge>,
    image_offset_mm: f32,
    px_per_mm: f32,
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
        picker_kind: None,
        picker_paths: Vec::new(),
        bridge_edits: BTreeMap::new(),
        undo: Vec::new(),
        layer_hit: None,
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
        focus_on_subject: ui.get_focus_on_subject(),
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
                    let pct = used as f64 * 100.0 / total as f64;
                    if pct < 0.5 {
                        "<1%".into()
                    } else {
                        format!("{pct:.0}%").into()
                    }
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
            layer: (ui.get_mode() == 0 && ui.get_view_mode() == 4).then(|| worker::LayerRequest {
                index: ui.get_layer_index().max(0) as usize,
                options: stencil_options(ui, st),
            }),
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

    let chips: Vec<LayerChip> = r
        .layers
        .iter()
        .enumerate()
        .map(|(i, (_, c, name))| LayerChip {
            color: rgb_to_slint(*c),
            label: format!("{} {name}", i + 1).into(),
        })
        .collect();
    ui.set_layer_chips(ModelRc::new(VecModel::from(chips)));
    if let Some(l) = r.layer {
        ui.set_layer_index(l.index as i32);
        ui.set_layer_image(Image::from_rgba8(l.image));
        let manual = l.bridges.iter().filter(|b| b.manual).count();
        let mut info = format!(
            "Layer {} · {} · {} bridge{} ({} added by you)",
            l.index + 1,
            l.name,
            l.bridges.len(),
            if l.bridges.len() == 1 { "" } else { "s" },
            manual
        );
        if l.floating > 0 {
            info += &format!(
                " · ⚠ {} floating piece{}",
                l.floating,
                if l.floating == 1 { "" } else { "s" }
            );
        }
        if l.dropped > 0 {
            info += &format!(
                " · {} tiny piece{} dropped",
                l.dropped,
                if l.dropped == 1 { "" } else { "s" }
            );
        }
        ui.set_layer_info(info.into());
        with_state(|st| {
            st.layer_hit = Some(LayerHit {
                palette_index: l.palette_index,
                bridges: l.bridges,
                image_offset_mm: l.image_offset_mm,
                px_per_mm: r.px_per_mm,
            })
        });
    }
}

/// Stencil sheet options from the UI, including the user's bridge edits.
fn stencil_options(ui: &AppWindow, st: &State) -> StencilOptions {
    let thickness = ui
        .get_thickness_names()
        .row_data(ui.get_thickness_index().max(0) as usize)
        .and_then(|s| s.trim_end_matches(" mm").parse::<f32>().ok())
        .unwrap_or(1.0);
    StencilOptions {
        thickness_mm: thickness,
        frame_mm: ui.get_frame_width(),
        registration_marks: ui.get_registration_marks(),
        bridge_islands: ui.get_bridge_islands(),
        bridge_width_mm: ui.get_bridge_width(),
        min_island_mm2: ui.get_min_island(),
        edits: st.bridge_edits.clone(),
        ..Default::default()
    }
}

/// A click in the Layers view at image pixel (x, y): toggles the bridge under the
/// cursor, or adds a bridge across the cut there.
fn layer_click(ui: &AppWindow, x: f32, y: f32) {
    let msg = with_state(|st| {
        let hit = st.layer_hit.as_ref()?;
        let p = [
            x / hit.px_per_mm + hit.image_offset_mm,
            y / hit.px_per_mm + hit.image_offset_mm,
        ];
        let color = hit.palette_index;
        // generous margin so thin bridges are easy to hit with a finger
        let margin = (0.8f32).max(6.0 / hit.px_per_mm);
        let under = hit
            .bridges
            .iter()
            .filter(|b| b.contains(p, margin))
            .min_by(|a, b| centre_dist(a, p).total_cmp(&centre_dist(b, p)))
            .copied();
        st.undo.push(st.bridge_edits.clone());
        let edits = st.bridge_edits.entry(color).or_default();
        Some(match under {
            Some(b) if b.manual => {
                // drop the added point that produced this bridge
                if let Some(i) = (0..edits.added.len())
                    .min_by(|&i, &j| dist(edits.added[i], centre(&b)).total_cmp(&dist(edits.added[j], centre(&b))))
                {
                    edits.added.remove(i);
                }
                "Removed your bridge"
            }
            Some(b) => {
                edits.removed.push(centre(&b));
                "Removed bridge (Ctrl+Z to undo)"
            }
            None => {
                edits.added.push(p);
                "Added a bridge across the cut (click it again to remove)"
            }
        })
    });
    if let Some(m) = msg {
        ui.set_can_undo(true);
        ui.set_status_text(m.into());
        request_processing(ui);
    }
}

fn centre(b: &stencil_core::stencil::Bridge) -> [f32; 2] {
    [(b.rect_mm[0] + b.rect_mm[2]) / 2.0, (b.rect_mm[1] + b.rect_mm[3]) / 2.0]
}
fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}
fn centre_dist(b: &stencil_core::stencil::Bridge, p: [f32; 2]) -> f32 {
    dist(centre(b), p)
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
    ui.set_status_text(format!("Loading {}…", platform::pretty(path)).into());
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
            ui.set_status_text(format!("Opened {}", platform::pretty(path)).into());
            with_state(|st| {
                st.source = Some(img);
                st.working = Some(Arc::new(working));
                st.bridge_edits.clear();
                st.undo.clear();
                st.layer_hit = None;
            });
            ui.set_can_undo(false);
            ui.set_layer_index(0);
            request_processing(ui);
        }
        Err(e) => {
            ui.set_status_text(format!("Could not open image: {e}").into());
            show_message(
                ui,
                "Open Image",
                &format!("Could not open {}:\n{e}", platform::pretty(path)),
                true,
            );
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
    let Some(path) = platform::save_path(&format!("Save {}", doc.title()), "PDF", "pdf", doc.file_name()) else {
        return;
    };
    match std::fs::write(&path, calibration_pdf(ui, doc)) {
        Ok(()) => ui.set_status_text(format!("Saved {} to {}", doc.title(), platform::pretty(&path)).into()),
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
        let msg = match platform::print_pdf(&path) {
            Ok(m) => {
                let t = doc.title();
                format!("{}{}: {m}", t[..1].to_uppercase(), &t[1..])
            }
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
    let Some(target) = platform::save_path(
        &format!("Export Stencil Layers ({})", format.label()),
        format.label(),
        format.ext(),
        &format!("{stem}-stencil.{}", format.ext()),
    ) else {
        return;
    };
    let options = with_state(|st| stencil_options(ui, st));
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
                        .map(platform::pretty)
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
                    show_message(&ui, "Export failed", &e, true);
                }
            }
        });
    });
}

fn load_palette_path(ui: &AppWindow, path: &Path) {
    match Palette::load(path) {
        Ok(p) => {
            with_state(|st| {
                st.library.push(p);
                ui.set_palette_source(st.library.len() as i32);
                ui.set_selected_swatch(-1);
                sync_palette_ui(ui, st);
            });
            ui.set_status_text(format!("Loaded palette {}", platform::pretty(path)).into());
            request_processing(ui);
        }
        Err(e) => ui.set_status_text(format!("Could not load palette: {e}").into()),
    }
}

/// Error / info message: native dialog on desktop, in-app sheet on mobile.
fn show_message(ui: &AppWindow, title: &str, text: &str, error: bool) {
    if platform::NATIVE_DIALOGS {
        platform::message(title, text, error);
    } else {
        ui.set_alert_title(title.into());
        ui.set_alert_text(text.into());
        ui.set_alert_visible(true);
    }
}

/// In-app file browser (used where native file dialogs are unavailable).
fn show_picker(ui: &AppWindow, kind: PickKind) {
    let granted = platform::ensure_media_permission();
    let paths = platform::list_files(kind);
    let items: Vec<PickerItem> = paths
        .iter()
        .map(|p| PickerItem {
            name: p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
                .into(),
            detail: p.parent().map(platform::pretty).unwrap_or_default().into(),
        })
        .collect();
    let empty = match (kind, granted) {
        (PickKind::Image, false) => "Allow StencilStudio to access your photos when Android asks, then tap Refresh.",
        (PickKind::Image, true) => "No images found in Pictures, DCIM, Download or Documents.",
        (PickKind::Palette, _) => "No palette files (.json) found in Documents or Download.",
    };
    ui.set_picker_title(kind.title().into());
    ui.set_picker_empty_text(empty.into());
    ui.set_picker_items(ModelRc::new(VecModel::from(items)));
    ui.set_picker_visible(true);
    with_state(|st| {
        st.picker_kind = Some(kind);
        st.picker_paths = paths;
    });
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: slint::android::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("StencilStudio"),
    );
    std::panic::set_hook(Box::new(|info| log::error!("panic: {info}")));
    let _ = platform::APP.set(app.clone());
    slint::android::init(app).expect("Slint Android backend");
    if let Err(e) = run(None) {
        log::error!("StencilStudio exited with error: {e}");
    }
}

const SHORTCUTS: &str = "\
Ctrl+O\tOpen image
Ctrl+Shift+O\tLoad palette
Ctrl+Shift+S\tSave palette
Ctrl+E\tExport stencil layers (SVG)
Ctrl+P\tPrint manual color chart
Ctrl+1 / 2 / 3 / 4\tOriginal / Preview / Outlines / Split
Ctrl+5\tStencil layers (click to add/remove bridges)
Ctrl+Z\tUndo bridge edit
Ctrl+= / Ctrl+-\tZoom in / out (or Ctrl+scroll)
Ctrl+0\tFit to window
Ctrl+Alt+0\tActual pixels
Ctrl+] / Ctrl+[\tMore / fewer colors
Ctrl+Alt+S / P\tStencil / Paint-by-Numbers mode
Ctrl+Alt+L / R\tToggle palette / properties panel
Ctrl+F1\tCollapse ribbon
Drag\tPan the canvas

On macOS, Ctrl means ⌘ Command.";

/// Build the UI and run the event loop. `initial` is an image to open on start.
pub fn run(initial: Option<PathBuf>) -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    ui.set_native_dialogs(platform::NATIVE_DIALOGS);
    if cfg!(target_os = "android") {
        // Narrow screens: start with the properties panel hidden; it can be toggled in View.
        ui.set_show_right_panel(false);
        ui.set_show_menu_bar(false);
        platform::ensure_media_permission();
    }

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
        if platform::NATIVE_DIALOGS {
            if let Some(path) = platform::pick_file(PickKind::Image) {
                open_image_path(&ui, &path);
            }
        } else {
            show_picker(&ui, PickKind::Image);
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
        if platform::NATIVE_DIALOGS {
            if let Some(path) = platform::pick_file(PickKind::Palette) {
                load_palette_path(&ui, &path);
            }
        } else {
            show_picker(&ui, PickKind::Palette);
        }
    });

    let w = ui.as_weak();
    ui.on_save_palette(move || {
        let ui = w.unwrap();
        let palette = with_state(|st| current_palette(&ui, st).unwrap_or_else(|| st.auto_palette.clone()));
        let Some(path) = platform::save_path("Save Palette", "Palette", "json", &format!("{}.json", palette.name))
        else {
            return;
        };
        match palette.save(&path) {
            Ok(()) => ui.set_status_text(format!("Saved palette to {}", platform::pretty(&path)).into()),
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

    let w = ui.as_weak();
    ui.on_show_about(move || {
        let text = format!(
                "StencilStudio {}\n\nTurn photos into multi-layer stencils and paint-by-numbers templates, matched to the colors you actually own.\n\nMade with Slint (https://slint.dev), used under the Slint Royalty-free License.",
                env!("CARGO_PKG_VERSION")
            );
        show_message(&w.unwrap(), "About StencilStudio", &text, false);
    });

    let w = ui.as_weak();
    ui.on_show_shortcuts(move || show_message(&w.unwrap(), "Keyboard Shortcuts", SHORTCUTS, false));

    let w = ui.as_weak();
    ui.on_picker_chosen(move |i| {
        let ui = w.unwrap();
        ui.set_picker_visible(false);
        let (kind, path) = with_state(|st| (st.picker_kind, st.picker_paths.get(i as usize).cloned()));
        match (kind, path) {
            (Some(PickKind::Image), Some(p)) => open_image_path(&ui, &p),
            (Some(PickKind::Palette), Some(p)) => load_palette_path(&ui, &p),
            _ => {}
        }
    });
    let w = ui.as_weak();
    ui.on_picker_refresh(move || {
        let ui = w.unwrap();
        if let Some(kind) = with_state(|st| st.picker_kind) {
            show_picker(&ui, kind);
        }
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
    ui.on_layer_selected(move |i| {
        let ui = w.unwrap();
        ui.set_layer_index(i);
        request_processing(&ui);
    });
    let w = ui.as_weak();
    ui.on_canvas_clicked(move |x, y| {
        let ui = w.unwrap();
        if ui.get_mode() == 0 && ui.get_view_mode() == 4 {
            layer_click(&ui, x, y);
        }
    });
    let w = ui.as_weak();
    ui.on_stencil_options_changed(move || {
        let ui = w.unwrap();
        if ui.get_mode() == 0 && ui.get_view_mode() == 4 {
            request_processing(&ui);
        }
    });
    let w = ui.as_weak();
    ui.on_reset_layer_bridges(move || {
        let ui = w.unwrap();
        let changed = with_state(|st| {
            let color = st.layer_hit.as_ref()?.palette_index;
            st.undo.push(st.bridge_edits.clone());
            st.bridge_edits.remove(&color);
            Some(())
        });
        if changed.is_some() {
            ui.set_can_undo(true);
            ui.set_status_text("Bridges on this layer reset to automatic".into());
            request_processing(&ui);
        }
    });
    let w = ui.as_weak();
    ui.on_undo(move || {
        let ui = w.unwrap();
        let more = with_state(|st| {
            let prev = st.undo.pop()?;
            st.bridge_edits = prev;
            Some(!st.undo.is_empty())
        });
        if let Some(more) = more {
            ui.set_can_undo(more);
            ui.set_status_text("Undid bridge edit".into());
            request_processing(&ui);
        }
    });

    let w = ui.as_weak();
    ui.on_not_yet(move |what| {
        w.unwrap()
            .set_status_text(format!("{what} is coming in a later milestone").into())
    });

    if let Some(path) = initial {
        open_image_path(&ui, &path);
    }

    ui.run()
}
