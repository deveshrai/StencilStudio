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
        remove_background: std::env::var("REMOVE_BG").is_ok(),
        ..Default::default()
    };

    let t = std::time::Instant::now();
    let r = pipeline::process(&img, &palette, &settings);
    let t_proc = t.elapsed();
    let sb = SmoothBoundaries::build(&r.segmentation, r.sigma_px(&settings));
    let t_vec = t.elapsed();
    let smooth = render::render_smooth_fill(&r.segmentation, &sb, &r.palette);
    let lines = render::render_smooth_outlines(&r.segmentation, &sb, Rgb8::new(30, 30, 30), 1.2);
    let t_render = t.elapsed();
    let opts = StencilOptions::default();
    let layers = build_layers(&r.segmentation, &sb, &r.palette, r.px_per_mm, &opts);
    let t_layers = t.elapsed();
    println!(
        "{:?}: process {t_proc:?}, vectors {:?}, render {:?}, layers {:?}; {} regions",
        img.dimensions(),
        t_vec - t_proc,
        t_render - t_vec,
        t_layers - t_render,
        r.segmentation.region_count()
    );
    render::render_reduced(&r.segmentation, &r.palette)
        .save(out.join("reduced.png"))
        .unwrap();
    smooth.save(out.join("smooth.png")).unwrap();
    lines.save(out.join("outlines.png")).unwrap();
    for l in &layers {
        let base = format!("layer{}", l.number);
        std::fs::write(out.join(format!("{base}.svg")), export::layer_svg(l, &opts)).unwrap();
        std::fs::write(out.join(format!("{base}.dxf")), export::layer_dxf(l, &opts)).unwrap();
        let stl = export::layer_stl(l, &opts);
        println!("{base} {}: manifold {}", l.name, manifold(&stl));
        std::fs::write(out.join(format!("{base}.stl")), stl).unwrap();
        layer_png(l, 4.0).save(out.join(format!("{base}.png"))).unwrap();
        println!(
            "  layer {} {:<16} bridged {:>3} dropped {:>3} left {}",
            l.number,
            l.name,
            l.bridged,
            l.dropped,
            l.island_count()
        );
    }
}

/// Render a layer's sheet material (dark) with cut-outs (white) at `px_per_mm`.
fn layer_png(l: &stencil_core::stencil::StencilLayer, px_per_mm: f32) -> image::RgbaImage {
    let (w, h) = (
        (l.sheet_mm.0 * px_per_mm).ceil() as u32,
        (l.sheet_mm.1 * px_per_mm).ceil() as u32,
    );
    let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
    pm.fill(tiny_skia::Color::WHITE);
    let mut pb = tiny_skia::PathBuilder::new();
    for ring in l.cut_paths() {
        pb.move_to(ring[0][0], ring[0][1]);
        for p in &ring[1..] {
            pb.line_to(p[0], p[1]);
        }
        pb.close();
    }
    if let Some(path) = pb.finish() {
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(58, 61, 70, 255);
        paint.anti_alias = true;
        let tf = tiny_skia::Transform::from_scale(px_per_mm, px_per_mm);
        pm.fill_path(&path, &paint, tiny_skia::FillRule::EvenOdd, tf, None);
    }
    image::RgbaImage::from_raw(w, h, pm.take_demultiplied()).unwrap()
}

/// Closed 2-manifold check: every directed edge is matched by its reverse exactly once.
fn manifold(stl: &[u8]) -> bool {
    let n = u32::from_le_bytes(stl[80..84].try_into().unwrap()) as usize;
    let mut edges = std::collections::HashMap::<_, i32>::new();
    for i in 0..n {
        let base = 84 + i * 50 + 12;
        let p = |v: usize| {
            let f = |k: usize| u32::from_le_bytes(stl[base + (v * 3 + k) * 4..base + (v * 3 + k) * 4 + 4].try_into().unwrap());
            (f(0), f(1), f(2))
        };
        for v in 0..3 {
            *edges.entry((p(v), p((v + 1) % 3))).or_default() += 1;
        }
    }
    edges.iter().all(|((a, b), c)| *c == 1 && edges.get(&(*b, *a)) == Some(&1))
}
