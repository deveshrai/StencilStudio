//! Dev tool: read a photographed / scanned calibration sheet.
//! `cargo run -p stencil-core --release --example scan_read -- <photo.jpg|png>`
//! With `--fill <rendered-sheet.png> <out.png>` it first paints test colours into a
//! rendered (A4) sheet, rotates it 90° and adds a warm cast, then reads that back.
use stencil_core::Rgb8;
use stencil_core::calibration;
use stencil_core::paper::PaperSize;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let img = if a[1] == "--fill" {
        let mut img = on_white(image::open(&a[2]).expect("open"));
        let ppm = img.width() as f32 / 210.0;
        let l = calibration::layout(&PaperSize::new("A4", 210.0, 297.0));
        let paints = [
            "C8231E", "2A3A9A", "F6C800", "3E6B2A", "8A4A26", "1E1E1E", "EE7A1C", "F4F4F0",
        ];
        for (b, hex) in l.boxes.iter().zip(paints) {
            let c = Rgb8::from_hex(hex).unwrap();
            for y in ((b.y + 1.0) * ppm) as u32..((b.y + b.h - 1.0) * ppm) as u32 {
                for x in ((b.x + 1.0) * ppm) as u32..((b.x + b.w - 1.0) * ppm) as u32 {
                    img.put_pixel(x, y, image::Rgb([c.r, c.g, c.b]));
                }
            }
        }
        let img = image::imageops::rotate90(&img);
        let mut big = image::RgbImage::from_pixel(img.width() + 300, img.height() + 200, image::Rgb([70, 60, 50]));
        image::imageops::overlay(&mut big, &img, 150, 100);
        for p in big.pixels_mut() {
            *p = image::Rgb([
                (p[0] as f32 * 0.95) as u8,
                (p[1] as f32 * 0.85) as u8,
                (p[2] as f32 * 0.7) as u8,
            ]);
        }
        big.save(&a[3]).unwrap();
        println!("expected: {paints:?} (last = white paint, may read as empty)");
        big
    } else {
        stencil_core::scan::load_scan(std::path::Path::new(&a[1])).expect("load")
    };
    match stencil_core::scan::read_sheet(&img) {
        Ok(r) => {
            println!("paper {}, paper white {:?}", r.paper, r.paper_white);
            for (i, c) in r.colors.iter().enumerate() {
                if let Some(c) = c {
                    println!("box {:2}: {}", i + 1, c.to_hex());
                }
            }
        }
        Err(e) => println!("error: {e}"),
    }
}

/// Flatten transparency onto white paper.
fn on_white(img: image::DynamicImage) -> image::RgbImage {
    let rgba = img.to_rgba8();
    image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let p = rgba.get_pixel(x, y);
        let a = p[3] as f32 / 255.0;
        image::Rgb([0, 1, 2].map(|k| (p[k] as f32 * a + 255.0 * (1.0 - a)).round() as u8))
    })
}
