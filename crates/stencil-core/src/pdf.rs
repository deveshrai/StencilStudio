//! A tiny dependency-free PDF writer (vector shapes + base-14 Helvetica text).
//! Coordinates passed in are millimetres from the top-left corner.

use std::fmt::Write as _;

const PT_PER_MM: f32 = 72.0 / 25.4;

pub struct Page {
    w_mm: f32,
    h_mm: f32,
    ops: String,
}

#[derive(Clone, Copy)]
pub enum Font {
    Regular,
    Bold,
}

impl Page {
    pub fn new(w_mm: f32, h_mm: f32) -> Self {
        Self {
            w_mm,
            h_mm,
            ops: String::new(),
        }
    }

    fn x(&self, mm: f32) -> f32 {
        mm * PT_PER_MM
    }
    fn y(&self, mm: f32) -> f32 {
        (self.h_mm - mm) * PT_PER_MM
    }

    pub fn fill_rgb(&mut self, r: u8, g: u8, b: u8) -> &mut Self {
        let _ = writeln!(
            self.ops,
            "{:.3} {:.3} {:.3} rg",
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0
        );
        self
    }

    pub fn stroke_rgb(&mut self, r: u8, g: u8, b: u8) -> &mut Self {
        let _ = writeln!(
            self.ops,
            "{:.3} {:.3} {:.3} RG",
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0
        );
        self
    }

    pub fn line_width(&mut self, mm: f32) -> &mut Self {
        let _ = writeln!(self.ops, "{:.3} w", mm * PT_PER_MM);
        self
    }

    pub fn dash(&mut self, on_mm: f32, off_mm: f32) -> &mut Self {
        if on_mm <= 0.0 {
            self.ops.push_str("[] 0 d\n");
        } else {
            let _ = writeln!(self.ops, "[{:.2} {:.2}] 0 d", on_mm * PT_PER_MM, off_mm * PT_PER_MM);
        }
        self
    }

    /// Rectangle with top-left at (x, y) in mm.
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, fill: bool, stroke: bool) -> &mut Self {
        let _ = write!(
            self.ops,
            "{:.3} {:.3} {:.3} {:.3} re ",
            self.x(x),
            self.y(y + h),
            w * PT_PER_MM,
            h * PT_PER_MM
        );
        self.ops.push_str(match (fill, stroke) {
            (true, true) => "B\n",
            (true, false) => "f\n",
            _ => "S\n",
        });
        self
    }

    pub fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32) -> &mut Self {
        let _ = writeln!(
            self.ops,
            "{:.3} {:.3} m {:.3} {:.3} l S",
            self.x(x1),
            self.y(y1),
            self.x(x2),
            self.y(y2)
        );
        self
    }

    /// Text with its baseline at (x, y) mm. `size` is in points.
    pub fn text(&mut self, x: f32, y: f32, size_pt: f32, font: Font, s: &str) -> &mut Self {
        let f = match font {
            Font::Regular => "F1",
            Font::Bold => "F2",
        };
        let _ = writeln!(
            self.ops,
            "BT /{f} {size_pt:.2} Tf {:.3} {:.3} Td ({}) Tj ET",
            self.x(x),
            self.y(y),
            escape(s)
        );
        self
    }

    pub fn width_mm(&self) -> f32 {
        self.w_mm
    }
}

/// Approximate Helvetica text width in mm (for centring/right-aligning).
pub fn text_width_mm(s: &str, size_pt: f32) -> f32 {
    s.chars().count() as f32 * 0.52 * size_pt / PT_PER_MM
}

/// Escape a string for a PDF literal using WinAnsi (Latin-1 subset) encoding.
fn escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            ' '..='~' => out.push(c),
            c if (c as u32) >= 0xA0 && (c as u32) <= 0xFF => {
                let _ = write!(out, "\\{:03o}", c as u32);
            }
            '–' | '—' => out.push('-'),
            '·' => out.push_str("\\267"),
            _ => out.push('?'),
        }
    }
    out
}

pub fn write_pdf(pages: &[Page], title: &str) -> Vec<u8> {
    let mut objs: Vec<String> = Vec::new();
    // 1 catalog, 2 pages, 3 F1, 4 F2, 5 info, then (page, content) pairs
    let n = pages.len();
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 6 + i * 2)).collect();
    objs.push("<< /Type /Catalog /Pages 2 0 R >>".into());
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into());
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>".into());
    objs.push(format!("<< /Title ({}) /Producer (StencilStudio) >>", escape(title)));
    for (i, p) in pages.iter().enumerate() {
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {:.3} {:.3}] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {} 0 R >>",
            p.w_mm * PT_PER_MM,
            p.h_mm * PT_PER_MM,
            7 + i * 2
        ));
        objs.push(format!("<< /Length {} >>\nstream\n{}endstream", p.ops.len(), p.ops));
    }
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(objs.len());
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    let mut tail = format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1);
    for off in offsets {
        let _ = writeln!(tail, "{off:010} 00000 n ");
    }
    let _ = write!(
        tail,
        "trailer\n<< /Size {} /Root 1 0 R /Info 5 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objs.len() + 1
    );
    out.extend_from_slice(tail.as_bytes());
    out
}
