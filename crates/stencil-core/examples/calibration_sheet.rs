//! Dev CLI: `cargo run -p stencil-core --example calibration_sheet -- out.pdf [letter]`
use stencil_core::{Palette, PaperSize, calibration};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "calibration.pdf".into());
    let paper = if std::env::args().nth(2).as_deref() == Some("letter") {
        PaperSize::new("US Letter", 215.9, 279.4)
    } else {
        PaperSize::new("A4", 210.0, 297.0)
    };
    let p = Palette::basic_acrylic_12();
    let names: Vec<String> = p.swatches.iter().map(|s| s.name.clone()).collect();
    std::fs::write(&out, calibration::sheet_pdf(&paper, &p.name, &names)).unwrap();
}
