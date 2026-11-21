//! Dev tool: `cargo run -p stencil-studio --release --example subject -- <in> <out.png> [model]`
//! Writes the image with the removed background in magenta. Pass `model` to skip
//! Apple Vision and use the bundled network (what Android uses).
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let img = image::open(&a[1]).expect("open").to_rgb8();
    let img = stencil_core::pipeline::fit_within(&img, 1600);
    if a.get(3).map(String::as_str) == Some("model") {
        unsafe { std::env::set_var("STENCIL_NO_VISION", "1") };
    }
    let t = std::time::Instant::now();
    let (m, src) = stencil_studio::subject::foreground_mask(&img);
    eprintln!("{:?} {:?} in {:?}", img.dimensions(), src, t.elapsed());
    let mut out = img.clone();
    for (p, &f) in out.pixels_mut().zip(&m) {
        if !f {
            *p = image::Rgb([255, 0, 255]);
        }
    }
    out.save(&a[2]).unwrap();
}
