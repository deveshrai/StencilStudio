//! Printable palette calibration sheet.
//!
//! The layout is defined here once so the (upcoming) photo/scan importer can
//! locate the fiducials, the gray reference strip and each swatch box using
//! exactly the same geometry.

use crate::paper::PaperSize;
use crate::pdf::{Font, Page, text_width_mm, write_pdf};

pub const SHEET_VERSION: u32 = 1;
pub const COLUMNS: usize = 4;
pub const ROWS: usize = 6;
pub const SWATCH_COUNT: usize = COLUMNS * ROWS;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectMm {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RectMm {
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

#[derive(Debug, Clone)]
pub struct SheetLayout {
    pub page_mm: (f32, f32),
    /// Corner markers: top-left, top-right, bottom-left, bottom-right.
    /// The bottom-right one is hollow so the sheet's orientation is unambiguous.
    pub fiducials: [RectMm; 4],
    /// Neutral reference patches and their printed gray level (255 = paper white).
    pub gray_strip: Vec<(RectMm, u8)>,
    /// Swatch boxes in reading order (box 1 first).
    pub boxes: Vec<RectMm>,
}

pub const FIDUCIAL_MM: f32 = 10.0;

pub fn layout(paper: &PaperSize) -> SheetLayout {
    let (pw, ph) = (paper.width_mm, paper.height_mm);
    let m = 10.0;
    let f = FIDUCIAL_MM;
    let fid = |x: f32, y: f32| RectMm { x, y, w: f, h: f };
    let fiducials = [
        fid(m, m),
        fid(pw - m - f, m),
        fid(m, ph - m - f),
        fid(pw - m - f, ph - m - f),
    ];

    let levels = [255u8, 230, 191, 128, 64, 26, 0];
    let patch_w = 14.0;
    let strip_x = (pw - patch_w * levels.len() as f32) / 2.0;
    let gray_strip = levels
        .iter()
        .enumerate()
        .map(|(i, &g)| {
            (
                RectMm {
                    x: strip_x + i as f32 * patch_w,
                    y: 52.0,
                    w: patch_w,
                    h: 9.0,
                },
                g,
            )
        })
        .collect();

    let top = 74.0;
    let bottom = ph - m - f - 4.0;
    let left = m + 4.0;
    let pitch_x = (pw - 2.0 * left) / COLUMNS as f32;
    let pitch_y = (bottom - top) / ROWS as f32;
    let mut boxes = Vec::with_capacity(SWATCH_COUNT);
    for r in 0..ROWS {
        for c in 0..COLUMNS {
            boxes.push(RectMm {
                x: left + c as f32 * pitch_x + 2.0,
                y: top + r as f32 * pitch_y,
                w: pitch_x - 4.0,
                h: pitch_y - 9.0,
            });
        }
    }
    SheetLayout {
        page_mm: (pw, ph),
        fiducials,
        gray_strip,
        boxes,
    }
}

/// Render the calibration sheet as a PDF. `names` pre-fills swatch labels.
pub fn sheet_pdf(paper: &PaperSize, palette_name: &str, names: &[String]) -> Vec<u8> {
    let l = layout(paper);
    let (pw, _ph) = l.page_mm;
    let mut p = Page::new(l.page_mm.0, l.page_mm.1);

    // Fiducials
    p.fill_rgb(0, 0, 0);
    for (i, r) in l.fiducials.iter().enumerate() {
        p.rect(r.x, r.y, r.w, r.h, true, false);
        if i == 3 {
            p.fill_rgb(255, 255, 255)
                .rect(r.x + 2.5, r.y + 2.5, r.w - 5.0, r.h - 5.0, true, false)
                .fill_rgb(0, 0, 0);
        }
    }

    // Header
    let title = "Palette Calibration Sheet";
    p.fill_rgb(20, 20, 20)
        .text((pw - text_width_mm(title, 18.0)) / 2.0, 19.0, 18.0, Font::Bold, title);
    let sub = if palette_name.is_empty() {
        "StencilStudio".to_string()
    } else {
        format!("StencilStudio  \u{b7}  {palette_name}")
    };
    p.fill_rgb(90, 90, 90)
        .text((pw - text_width_mm(&sub, 10.0)) / 2.0, 26.0, 10.0, Font::Regular, &sub);
    let lines = [
        "1. Fill each box completely with one of your paints, pens or pencils, and write its name on the line.",
        "2. Let it dry, then photograph the whole sheet flat in even daylight (or scan it).",
        "3. Keep all four black corner markers and the gray strip visible. Import via Palette > Import Scan.",
    ];
    for (i, s) in lines.iter().enumerate() {
        p.fill_rgb(60, 60, 60)
            .text(24.0, 35.0 + i as f32 * 4.6, 8.5, Font::Regular, s);
    }

    // Gray reference strip
    p.line_width(0.2).stroke_rgb(0, 0, 0);
    for (r, g) in &l.gray_strip {
        p.fill_rgb(*g, *g, *g).rect(r.x, r.y, r.w, r.h, true, true);
    }
    let cap = "Reference strip - do not paint";
    let r0 = l.gray_strip[0].0;
    p.fill_rgb(110, 110, 110).text(
        (pw - text_width_mm(cap, 7.0)) / 2.0,
        r0.y + r0.h + 4.0,
        7.0,
        Font::Regular,
        cap,
    );

    // Swatch boxes
    for (i, b) in l.boxes.iter().enumerate() {
        p.line_width(0.35)
            .stroke_rgb(0, 0, 0)
            .dash(0.0, 0.0)
            .rect(b.x, b.y, b.w, b.h, false, true);
        let num = format!("{}", i + 1);
        p.fill_rgb(0, 0, 0).text(b.x, b.y + b.h + 5.0, 9.0, Font::Bold, &num);
        let name = names.get(i).map(String::as_str).unwrap_or("");
        let nx = b.x + 7.0;
        p.line_width(0.15)
            .stroke_rgb(150, 150, 150)
            .line(nx, b.y + b.h + 5.6, b.x + b.w, b.y + b.h + 5.6);
        if !name.is_empty() {
            p.fill_rgb(40, 40, 40)
                .text(nx + 0.5, b.y + b.h + 4.8, 8.0, Font::Regular, name);
        }
    }

    let foot = format!("Calibration sheet v{SHEET_VERSION}  \u{b7}  {}", paper.name);
    let fy = l.fiducials[2].y + FIDUCIAL_MM - 1.0;
    p.fill_rgb(130, 130, 130)
        .text((pw - text_width_mm(&foot, 7.0)) / 2.0, fy, 7.0, Font::Regular, &foot);

    write_pdf(&[p], "StencilStudio Palette Calibration Sheet")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_fits_on_a4_and_letter() {
        for paper in [
            PaperSize::new("A4", 210.0, 297.0),
            PaperSize::new("Letter", 215.9, 279.4),
        ] {
            let l = layout(&paper);
            assert_eq!(l.boxes.len(), SWATCH_COUNT);
            let last = l.boxes.last().unwrap();
            assert!(last.y + last.h + 6.0 < l.fiducials[2].y + FIDUCIAL_MM, "{}", paper.name);
            assert!(last.x + last.w < paper.width_mm - 10.0);
            assert!(l.boxes[0].w > 30.0 && l.boxes[0].h > 18.0, "{:?}", l.boxes[0]);
        }
    }

    #[test]
    fn pdf_is_well_formed() {
        let pdf = sheet_pdf(
            &PaperSize::new("A4", 210.0, 297.0),
            "My (Acrylics)",
            &["Cadmium Red".into()],
        );
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.starts_with("%PDF-1.4") && s.trim_end().ends_with("%%EOF"));
        assert!(s.contains("(Cadmium Red) Tj") && s.contains("My \\(Acrylics\\)"));
        // xref offset must point at the xref table
        let sx = s.rfind("startxref\n").unwrap();
        let off: usize = s[sx + 10..].lines().next().unwrap().parse().unwrap();
        assert_eq!(&pdf[off..off + 4], b"xref");
    }
}
