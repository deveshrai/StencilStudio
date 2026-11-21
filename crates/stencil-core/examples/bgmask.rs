//! Dev tool: `cargo run -p stencil-core --release --example bgmask -- <in> <out.png>`
//! Writes the image with the removed background painted magenta.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let img = image::open(&a[1]).expect("open").to_rgb8();
    let img = stencil_core::pipeline::fit_within(&img, 1000);
    let t = std::time::Instant::now();
    let m = stencil_core::background::foreground_mask(&img);
    eprintln!(
        "{:?} in {:?}, subject {:.0}%",
        img.dimensions(),
        t.elapsed(),
        100.0 * m.iter().filter(|&&v| v).count() as f32 / m.len() as f32
    );
    let mut out = img.clone();
    for (p, &f) in out.pixels_mut().zip(&m) {
        if !f {
            *p = image::Rgb([255, 0, 255]);
        }
    }
    out.save(&a[2]).unwrap();
}
