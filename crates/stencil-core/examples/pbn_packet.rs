//! Build a paint-by-numbers packet from a photo: `pbn_packet <photo> <out.pdf> [colours]`
use stencil_core::pbn::{PacketInput, packet_pdf};
use stencil_core::pipeline::{Mode, Settings, fit_within, process};
use stencil_core::vector::SmoothBoundaries;
use stencil_core::Palette;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let img = image::open(&a[1]).expect("image").to_rgb8();
    let img = fit_within(&img, 3000);
    let settings = Settings {
        mode: Mode::PaintByNumbers,
        color_count: a.get(3).and_then(|s| s.parse().ok()).unwrap_or(10),
        remove_background: std::env::var_os("REMOVE_BG").is_some(),
        background_color: stencil_core::Rgb8::new(20, 20, 20),
        ..Default::default()
    };
    let palette = Palette::basic_acrylic_12();
    let t = std::time::Instant::now();
    let r = process(&img, &palette, &settings);
    let sb = SmoothBoundaries::build(&r.segmentation, r.sigma_px(&settings));
    let pdf = packet_pdf(&PacketInput {
        seg: &r.segmentation,
        sb: &sb,
        palette: &r.palette,
        output: &settings.output,
        title: std::path::Path::new(&a[1]).file_name().unwrap().to_str().unwrap(),
    });
    std::fs::write(&a[2], &pdf).unwrap();
    println!(
        "{} regions, {} KB, {:.1} s",
        r.segmentation.region_count(),
        pdf.len() / 1024,
        t.elapsed().as_secs_f32()
    );
}
