//! Manual color chart: a printed "fan deck" of in-gamut color patches with grid
//! codes (e.g. `D7`). The user holds a dried dab of each real paint next to the
//! printed chart, finds the closest patch by eye, and types its code into the app.
//! Never scanned — matching against the *print* sidesteps screen/printer mismatch.

use crate::color::{Lab, Rgb8};
use crate::paper::PaperSize;
use crate::pdf::{Font, Page, text_width_mm, write_pdf};

pub const HUES: usize = 16;
/// Lightness steps used for both the vivid and the muted block.
pub const LIGHTNESS: [f32; 6] = [90.0, 78.0, 66.0, 54.0, 42.0, 30.0];
pub const ROWS: usize = LIGHTNESS.len() * 2 + 1;
pub const CHART_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct ChartPatch {
    /// Column letter A..P + row number 1..13, e.g. "D7".
    pub code: String,
    pub color: Rgb8,
    pub col: usize,
    pub row: usize,
}

/// Largest chroma at (L, h) that is still inside sRGB.
fn max_chroma(l: f32, h: f32) -> f32 {
    let (mut lo, mut hi) = (0.0f32, 160.0f32);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if Lab::from_lch(l, mid, h).in_srgb_gamut() {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

fn column_letter(col: usize) -> char {
    (b'A' + col as u8) as char
}

/// All patches, row-major. Rows 1–6: vivid (90% of max in-gamut chroma) from light to
/// dark; rows 7–12: muted (40%) — covers earth tones; row 13: neutral grays white→black.
pub fn patches() -> Vec<ChartPatch> {
    let mut out = Vec::with_capacity(HUES * ROWS);
    for row in 0..ROWS {
        for col in 0..HUES {
            let color = if row < 2 * LIGHTNESS.len() {
                let l = LIGHTNESS[row % LIGHTNESS.len()];
                let h = 20.0 + col as f32 * 360.0 / HUES as f32; // start at red
                let k = if row < LIGHTNESS.len() { 0.9 } else { 0.4 };
                Lab::from_lch(l, max_chroma(l, h) * k, h).to_rgb()
            } else {
                let l = 100.0 - col as f32 * 100.0 / (HUES - 1) as f32;
                Lab::new(l, 0.0, 0.0).to_rgb()
            };
            out.push(ChartPatch {
                code: format!("{}{}", column_letter(col), row + 1),
                color,
                col,
                row,
            });
        }
    }
    out
}

/// Look up a chart code such as "d7" or " D07 ".
pub fn lookup(code: &str) -> Option<Rgb8> {
    let code = code.trim().to_ascii_uppercase();
    let mut chars = code.chars();
    let letter = chars.next()?;
    let col = (letter as u32).checked_sub('A' as u32)? as usize;
    let row: usize = chars.as_str().trim().parse().ok()?;
    if col >= HUES || row == 0 || row > ROWS {
        return None;
    }
    Some(patches()[(row - 1) * HUES + col].color)
}

/// Closest chart code to a color (CIEDE2000).
pub fn nearest_code(c: Rgb8) -> String {
    let lab = c.to_lab();
    patches()
        .into_iter()
        .min_by(|a, b| {
            lab.delta_e2000(a.color.to_lab())
                .total_cmp(&lab.delta_e2000(b.color.to_lab()))
        })
        .map(|p| p.code)
        .unwrap_or_default()
}

/// Two pages: the color chart, and a "My paints" record sheet pre-filled with `names`.
pub fn chart_pdf(paper: &PaperSize, palette_name: &str, names: &[String]) -> Vec<u8> {
    let (pw, ph) = (paper.width_mm, paper.height_mm);
    let m = 12.0;

    // ---- Page 1: chart ----
    let mut p = Page::new(pw, ph);
    let title = "Manual Color Chart";
    p.fill_rgb(20, 20, 20).text(
        (pw - text_width_mm(title, 18.0)) / 2.0,
        m + 8.0,
        18.0,
        Font::Bold,
        title,
    );
    let lines = [
        "1. Paint a small dab of each of your colors (page 2 has boxes for this) and let it dry.",
        "2. In daylight, hold each dab next to this chart and find the closest patch.",
        "3. In StencilStudio select the color, type the patch code (e.g. D7) in \"Chart code\" and press Enter.",
        "Match against this printout, not your screen: printers and screens show colors differently.",
    ];
    for (i, s) in lines.iter().enumerate() {
        let (r, g, b) = if i == 3 { (150, 80, 0) } else { (60, 60, 60) };
        p.fill_rgb(r, g, b)
            .text(m + 2.0, m + 16.0 + i as f32 * 4.4, 8.3, Font::Regular, s);
    }

    let label_w = 7.0;
    let top = m + 38.0;
    let bottom = ph - m - 8.0;
    let pitch_x = (pw - 2.0 * m - label_w) / HUES as f32;
    let pitch_y = (bottom - top - 6.0) / ROWS as f32;
    let patch_w = pitch_x - 1.4;
    let patch_h = pitch_y - 3.6;
    let x0 = m + label_w;
    let y0 = top + 6.0;
    for col in 0..HUES {
        let s = column_letter(col).to_string();
        let cx = x0 + col as f32 * pitch_x + patch_w / 2.0;
        p.fill_rgb(0, 0, 0)
            .text(cx - text_width_mm(&s, 9.0) / 2.0, top + 3.5, 9.0, Font::Bold, &s);
    }
    p.line_width(0.1).stroke_rgb(170, 170, 170);
    for patch in patches() {
        let x = x0 + patch.col as f32 * pitch_x;
        let y = y0 + patch.row as f32 * pitch_y;
        if patch.col == 0 {
            let s = format!("{}", patch.row + 1);
            p.fill_rgb(0, 0, 0)
                .text(m, y + patch_h / 2.0 + 1.2, 9.0, Font::Bold, &s);
        }
        let c = patch.color;
        p.fill_rgb(c.r, c.g, c.b).rect(x, y, patch_w, patch_h, true, true);
        p.fill_rgb(90, 90, 90)
            .text(x + 0.3, y + patch_h + 2.4, 5.5, Font::Regular, &patch.code);
    }
    // Block labels
    let vivid_y = y0 + LIGHTNESS.len() as f32 * pitch_y - 0.25;
    p.line_width(0.3)
        .stroke_rgb(120, 120, 120)
        .line(x0, vivid_y, pw - m, vivid_y);
    let muted_y = y0 + 2.0 * LIGHTNESS.len() as f32 * pitch_y - 0.25;
    p.line(x0, muted_y, pw - m, muted_y);

    let foot = format!(
        "Rows 1-6 vivid, 7-12 muted/earth tones, 13 grays  \u{b7}  Chart v{CHART_VERSION}  \u{b7}  {}",
        paper.name
    );
    p.fill_rgb(130, 130, 130).text(
        (pw - text_width_mm(&foot, 7.0)) / 2.0,
        ph - m,
        7.0,
        Font::Regular,
        &foot,
    );

    // ---- Page 2: my paints ----
    let mut q = Page::new(pw, ph);
    let title = "My Paints";
    q.fill_rgb(20, 20, 20).text(
        (pw - text_width_mm(title, 18.0)) / 2.0,
        m + 8.0,
        18.0,
        Font::Bold,
        title,
    );
    let sub = if palette_name.is_empty() {
        String::new()
    } else {
        palette_name.to_string()
    };
    q.fill_rgb(90, 90, 90).text(
        (pw - text_width_mm(&sub, 10.0)) / 2.0,
        m + 14.5,
        10.0,
        Font::Regular,
        &sub,
    );
    q.fill_rgb(60, 60, 60).text(
        m + 2.0,
        m + 22.0,
        8.3,
        Font::Regular,
        "Dab each paint in its box, let it dry, then write the closest chart code next to it.",
    );
    let rows = 12;
    let top = m + 30.0;
    let row_h = (ph - m - 8.0 - top) / rows as f32;
    let col_w = (pw - 2.0 * m) / 2.0;
    for i in 0..rows * 2 {
        let (c, r) = (i / rows, i % rows);
        let x = m + c as f32 * col_w;
        let y = top + r as f32 * row_h;
        let num = format!("{}", i + 1);
        q.fill_rgb(0, 0, 0)
            .text(x, y + row_h / 2.0 + 1.0, 9.0, Font::Bold, &num);
        q.line_width(0.35)
            .stroke_rgb(0, 0, 0)
            .rect(x + 7.0, y + 1.5, 22.0, row_h - 3.0, false, true);
        let nx = x + 32.0;
        q.line_width(0.15).stroke_rgb(150, 150, 150).line(
            nx,
            y + row_h / 2.0 + 1.6,
            x + col_w - 24.0,
            y + row_h / 2.0 + 1.6,
        );
        if let Some(name) = names.get(i).filter(|n| !n.is_empty()) {
            q.fill_rgb(40, 40, 40)
                .text(nx + 0.5, y + row_h / 2.0 + 0.8, 8.5, Font::Regular, name);
        }
        q.fill_rgb(110, 110, 110).text(
            x + col_w - 22.0,
            y + row_h / 2.0 - 3.2,
            6.0,
            Font::Regular,
            "Chart code",
        );
        q.line_width(0.3)
            .stroke_rgb(0, 0, 0)
            .rect(x + col_w - 22.0, y + row_h / 2.0 - 2.0, 16.0, 6.5, false, true);
    }

    write_pdf(&[p, q], "StencilStudio Manual Color Chart")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chart_has_unique_in_gamut_patches() {
        let p = patches();
        assert_eq!(p.len(), HUES * ROWS);
        assert_eq!(p[0].code, "A1");
        assert_eq!(p.last().unwrap().code, "P13");
        assert_eq!(p.last().unwrap().color, Rgb8::BLACK);
        assert_eq!(p[12 * HUES].color, Rgb8::WHITE);
        // vivid patches are clearly more colorful than their muted counterparts,
        // and all patches are distinguishable
        let lab: Vec<Lab> = p.iter().map(|x| x.color.to_lab()).collect();
        let chroma = |l: Lab| (l.a * l.a + l.b * l.b).sqrt();
        for i in 0..HUES * LIGHTNESS.len() {
            let muted = i + HUES * LIGHTNESS.len();
            assert!(
                chroma(lab[i]) > chroma(lab[muted]) * 1.8,
                "{} vs {}",
                p[i].code,
                p[muted].code
            );
            assert!(chroma(lab[i]) > 8.0, "{} too dull", p[i].code);
        }
        for i in 0..p.len() {
            for j in i + 1..p.len() {
                assert!(lab[i].delta_e2000(lab[j]) > 1.0, "{} ~ {}", p[i].code, p[j].code);
            }
        }
    }

    #[test]
    fn lookup_and_nearest_roundtrip() {
        assert_eq!(lookup("d7"), Some(patches()[6 * HUES + 3].color));
        assert_eq!(lookup(" P13 "), Some(Rgb8::BLACK));
        assert_eq!(lookup("Q1"), None);
        assert_eq!(lookup("A14"), None);
        assert_eq!(lookup("A0"), None);
        for p in patches().iter().step_by(7) {
            assert_eq!(nearest_code(p.color), p.code);
        }
    }

    #[test]
    fn pdf_has_two_pages() {
        let pdf = chart_pdf(&PaperSize::new("A4", 210.0, 297.0), "Acrylics", &["Cadmium Red".into()]);
        let s = crate::pdf::readable(&pdf);
        assert!(s.contains("/Count 2"));
        assert!(s.contains("(Cadmium Red) Tj") && s.contains("(P13) Tj"));
    }
}
