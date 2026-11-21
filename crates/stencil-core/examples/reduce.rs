//! Dev CLI: `cargo run -p stencil-core --release --example reduce -- <in> <colors> <out.png> [palette.json]`
use stencil_core::{Palette, Settings, pipeline, render};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: reduce <in> <colors> <out.png> [palette.json]");
        std::process::exit(2);
    }
    let img = image::open(&args[1]).expect("open image").to_rgb8();
    let img = pipeline::fit_within(&img, 1600);
    let palette = args
        .get(4)
        .map(|p| Palette::load(p.as_ref()).expect("palette"))
        .unwrap_or_else(Palette::basic_acrylic_12);
    let settings = Settings {
        color_count: args[2].parse().expect("colors"),
        ..Default::default()
    };
    let t = std::time::Instant::now();
    let r = pipeline::process(&img, &palette, &settings);
    let names: Vec<&str> = r.selected.iter().map(|&i| palette.swatches[i].name.as_str()).collect();
    println!(
        "{:?} in {:?}: {} regions, selected {names:?}",
        img.dimensions(),
        t.elapsed(),
        r.segmentation.region_count()
    );
    render::render_reduced(&r.segmentation, &palette)
        .save(&args[3])
        .expect("save");
}
