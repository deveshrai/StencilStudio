//! Dev CLI: `cargo run -p stencil-core --release --example reduce -- <in> <colors> <out_dir> [palette.json]`
//! Writes reduced.png, smooth.png, outlines.png and per-layer SVG/DXF/STL files.
use stencil_core::stencil::{StencilOptions, build_layers};
use stencil_core::vector::SmoothBoundaries;
use stencil_core::{Palette, Rgb8, Settings, export, pipeline, render};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: reduce <in> <colors> <out_dir> [palette.json]");
        std::process::exit(2);
    }
    let out = std::path::Path::new(&args[3]);
    std::fs::create_dir_all(out).expect("out dir");
    let img = image::open(&args[1]).expect("open image").to_rgb8();
    let img = pipeline::fit_within(
        &img,
        std::env::var("MAXSIDE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1600),
    );
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
    let t_proc = t.elapsed();
    let sb = SmoothBoundaries::build(&r.segmentation, r.sigma_px(&settings));
    let t_vec = t.elapsed();
    let smooth = render::render_smooth_fill(&r.segmentation, &sb, &palette);
    let lines = render::render_smooth_outlines(&r.segmentation, &sb, Rgb8::new(30, 30, 30), 1.2);
    let t_render = t.elapsed();
    let opts = StencilOptions::default();
    let layers = build_layers(&r.segmentation, &sb, &palette, r.px_per_mm, &opts);
    let t_layers = t.elapsed();
    println!(
        "{:?}: process {t_proc:?}, vectors {:?}, render {:?}, layers {:?}; {} regions",
        img.dimensions(),
        t_vec - t_proc,
        t_render - t_vec,
        t_layers - t_render,
        r.segmentation.region_count()
    );
    render::render_reduced(&r.segmentation, &palette)
        .save(out.join("reduced.png"))
        .unwrap();
    smooth.save(out.join("smooth.png")).unwrap();
    lines.save(out.join("outlines.png")).unwrap();
    for l in &layers {
        let base = format!("layer{}", l.number);
        std::fs::write(out.join(format!("{base}.svg")), export::layer_svg(l, &opts)).unwrap();
        std::fs::write(out.join(format!("{base}.dxf")), export::layer_dxf(l, &opts)).unwrap();
        std::fs::write(out.join(format!("{base}.stl")), export::layer_stl(l, &opts)).unwrap();
        println!(
            "  layer {} {:<16} islands {:>3}  holes {:>3}",
            l.number,
            l.name,
            l.island_count(),
            l.pieces[0].holes.len()
        );
    }
}
