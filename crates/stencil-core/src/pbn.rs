//! Paint-by-numbers packet: a 4-page PDF.
//!
//! 1. Outlines only, to paint on.
//! 2. The finished picture: every area filled with its paint, outlines on top.
//! 3. Outlines with each area's paint number.
//! 4. Colour key: number, printed colour, an empty box to paint a test dab in, paint name.

use crate::color::Rgb8;
use crate::palette::Palette;
use crate::paper::OutputSize;
use crate::pdf::{Font, Page, text_width_mm, write_pdf};
use crate::regions::Segmentation;
use crate::vector::{Pt, SmoothBoundaries, color_contours, simplify_open};

const PT_PER_MM: f32 = 72.0 / 25.4;
const LINE: Rgb8 = Rgb8 { r: 110, g: 110, b: 110 };
const NUMBER: Rgb8 = Rgb8 { r: 85, g: 85, b: 85 };
const LINE_MM: f32 = 0.2;
const MIN_NUMBER_PT: f32 = 3.5;
const MAX_NUMBER_PT: f32 = 9.0;

/// One colour of the key.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyEntry {
    pub number: usize,
    pub palette_index: u16,
    pub name: String,
    pub color: Rgb8,
    /// Share of the picture's area, 0..1.
    pub share: f32,
}

/// Where a region's number goes: centre in image pixels and the radius of the
/// largest circle around it that stays inside the region.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Label {
    pub region: u32,
    pub at: Pt,
    pub radius_px: f32,
}

/// The colours to paint, numbered lightest first (the same order as the stencil
/// layers). A removed background is included only when it isn't white paper.
pub fn color_key(seg: &Segmentation, palette: &Palette) -> Vec<KeyEntry> {
    let mut order = crate::stencil::layer_order(seg, palette);
    if let Some(bg) = seg.background
        && seg.region_color.contains(&bg)
        && palette.swatches[bg as usize].lab().l < 97.0
    {
        order.push(bg);
    }
    let mut area = vec![0u64; palette.len()];
    for (r, &c) in seg.region_color.iter().enumerate() {
        area[c as usize] += seg.region_area[r] as u64;
    }
    let total = (seg.width as u64 * seg.height as u64).max(1) as f32;
    order
        .iter()
        .enumerate()
        .map(|(i, &c)| KeyEntry {
            number: i + 1,
            palette_index: c,
            name: palette.swatches[c as usize].name.clone(),
            color: palette.swatches[c as usize].color,
            share: area[c as usize] as f32 / total,
        })
        .collect()
}

/// The point deepest inside each region (by a chamfer distance transform), which is
/// where its number fits best — on the region's "spine" even for long, curved shapes.
pub fn label_positions(seg: &Segmentation) -> Vec<Label> {
    let (w, h) = (seg.width as usize, seg.height as usize);
    // Distance (×3, chamfer 3-4) to the nearest pixel of another region or the image edge.
    const INF: u32 = u32::MAX / 2;
    let mut d = vec![INF; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let l = seg.labels[i];
            let edge = x == 0
                || y == 0
                || x == w - 1
                || y == h - 1
                || seg.labels[i - 1] != l
                || seg.labels[i + 1] != l
                || seg.labels[i - w] != l
                || seg.labels[i + w] != l;
            if edge {
                d[i] = 3;
            }
        }
    }
    let relax = |d: &mut Vec<u32>, i: usize, j: usize, c: u32| {
        let v = d[j] + c;
        if v < d[i] {
            d[i] = v;
        }
    };
    for y in 1..h.saturating_sub(1) {
        for x in 1..w - 1 {
            let i = y * w + x;
            relax(&mut d, i, i - 1, 3);
            relax(&mut d, i, i - w, 3);
            relax(&mut d, i, i - w - 1, 4);
            relax(&mut d, i, i - w + 1, 4);
        }
    }
    for y in (1..h.saturating_sub(1)).rev() {
        for x in (1..w - 1).rev() {
            let i = y * w + x;
            relax(&mut d, i, i + 1, 3);
            relax(&mut d, i, i + w, 3);
            relax(&mut d, i, i + w + 1, 4);
            relax(&mut d, i, i + w - 1, 4);
        }
    }
    let mut best: Vec<(u32, usize)> = vec![(0, 0); seg.region_count()];
    for (i, (&l, &v)) in seg.labels.iter().zip(&d).enumerate() {
        let b = &mut best[l as usize];
        if v > b.0 {
            *b = (v, i);
        }
    }
    best.iter()
        .enumerate()
        .filter(|(_, b)| b.0 > 0)
        .map(|(r, &(v, i))| Label {
            region: r as u32,
            at: [(i % w) as f32 + 0.5, (i / w) as f32 + 0.5],
            radius_px: v as f32 / 3.0 - 0.5,
        })
        .collect()
}

/// Font size (pt) for `text` to fit inside a circle of `radius_mm`.
fn number_size(text: &str, radius_mm: f32) -> f32 {
    // Helvetica digits are 0.556 em wide, 0.72 em tall.
    let half_diag_em = 0.5 * ((text.len() as f32 * 0.556).powi(2) + 0.72f32.powi(2)).sqrt();
    (radius_mm * PT_PER_MM * 0.92 / half_diag_em).clamp(MIN_NUMBER_PT, MAX_NUMBER_PT)
}

pub struct PacketInput<'a> {
    pub seg: &'a Segmentation,
    pub sb: &'a SmoothBoundaries,
    pub palette: &'a Palette,
    pub output: &'a OutputSize,
    /// Shown in the footers and on the key page (usually the photo's file name).
    pub title: &'a str,
}

/// Page geometry: maps image pixels to mm on the page, centred in the printable area.
struct Frame {
    page: (f32, f32),
    origin: (f32, f32),
    mm_per_px: f32,
    margin: f32,
}

impl Frame {
    fn new(seg: &Segmentation, output: &OutputSize) -> Self {
        let page = if output.landscape {
            (output.paper.height_mm, output.paper.width_mm)
        } else {
            (output.paper.width_mm, output.paper.height_mm)
        };
        let (fw, fh) = output.fitted_mm(seg.width, seg.height);
        Self {
            page,
            origin: ((page.0 - fw) / 2.0, (page.1 - fh) / 2.0),
            mm_per_px: fw / seg.width as f32,
            margin: output.margin_mm,
        }
    }
    fn map(&self, p: Pt) -> Pt {
        [self.origin.0 + p[0] * self.mm_per_px, self.origin.1 + p[1] * self.mm_per_px]
    }
    fn map_all(&self, pts: &[Pt]) -> Vec<Pt> {
        pts.iter().map(|&p| self.map(p)).collect()
    }
    fn page(&self) -> Page {
        Page::new(self.page.0, self.page.1)
    }
    fn footer(&self, p: &mut Page, title: &str, text: &str) {
        if self.margin < 5.0 {
            return;
        }
        let y = self.page.1 - self.margin / 2.0 + 1.0;
        p.fill_rgb(120, 120, 120);
        p.text(self.margin, y, 7.5, Font::Regular, title);
        let w = text_width_mm(text, 7.5);
        p.text(self.page.0 - self.margin - w, y, 7.5, Font::Regular, text);
    }
}

fn outline_chains(sb: &SmoothBoundaries) -> Vec<Vec<Pt>> {
    sb.chains.iter().map(|c| simplify_open(c, 0.15)).collect()
}

fn draw_outlines(p: &mut Page, f: &Frame, chains: &[Vec<Pt>], line: Rgb8, width_mm: f32) {
    p.stroke_rgb(line.r, line.g, line.b).line_width(width_mm).round_lines();
    for c in chains {
        p.poly(&f.map_all(c), c.len() > 2 && c.first() == c.last());
        p.stroke();
    }
}

/// Build the 4-page packet.
pub fn packet_pdf(input: &PacketInput) -> Vec<u8> {
    let PacketInput {
        seg,
        sb,
        palette,
        output,
        title,
    } = *input;
    let f = Frame::new(seg, output);
    let key = color_key(seg, palette);
    let chains = outline_chains(sb);
    let mut pages = Vec::with_capacity(4);

    // 1. Outlines to paint.
    let mut p = f.page();
    draw_outlines(&mut p, &f, &chains, LINE, LINE_MM);
    f.footer(&mut p, title, "1 / 4 · Outlines to paint");
    pages.push(p);

    // 2. The coloured picture.
    let mut p = f.page();
    p.round_lines().line_width(0.12);
    let (fw, fh) = output.fitted_mm(seg.width, seg.height);
    let mut by_area: Vec<u16> = seg.region_color.clone();
    by_area.sort_unstable();
    by_area.dedup();
    let mut area = vec![0u64; palette.len()];
    for (r, &c) in seg.region_color.iter().enumerate() {
        area[c as usize] += seg.region_area[r] as u64;
    }
    by_area.sort_by_key(|&c| std::cmp::Reverse(area[c as usize]));
    if let Some(&c) = by_area.first() {
        let col = palette.swatches[c as usize].color;
        p.fill_rgb(col.r, col.g, col.b);
        p.rect(f.origin.0, f.origin.1, fw, fh, true, false);
    }
    for &c in by_area.iter().skip(1) {
        let col = palette.swatches[c as usize].color;
        p.fill_rgb(col.r, col.g, col.b).stroke_rgb(col.r, col.g, col.b);
        for l in color_contours(seg, sb, c, 0.15) {
            p.poly(&f.map_all(&l), true);
        }
        // the same-colour hairline hides the viewer's anti-aliasing seams
        p.fill_even_odd(true);
    }
    draw_outlines(&mut p, &f, &chains, Rgb8::new(60, 60, 60), 0.1);
    f.footer(&mut p, title, "2 / 4 · Finished picture");
    pages.push(p);

    // 3. Outlines with numbers.
    let mut p = f.page();
    draw_outlines(&mut p, &f, &chains, LINE, LINE_MM);
    let mut number_of = vec![0usize; palette.len()];
    for k in &key {
        number_of[k.palette_index as usize] = k.number;
    }
    p.fill_rgb(NUMBER.r, NUMBER.g, NUMBER.b);
    for l in label_positions(seg) {
        let n = number_of[seg.region_color[l.region as usize] as usize];
        if n == 0 {
            continue; // white background paper: nothing to paint
        }
        let text = n.to_string();
        let size = number_size(&text, l.radius_px * f.mm_per_px);
        let [x, y] = f.map(l.at);
        let cap = 0.72 * size / PT_PER_MM;
        p.text(x - text_width_mm_digits(&text, size) / 2.0, y + cap / 2.0, size, Font::Regular, &text);
    }
    f.footer(&mut p, title, "3 / 4 · Numbered outlines");
    pages.push(p);

    // 4. Colour key with test boxes.
    pages.push(key_page(&f, title, &key));
    write_pdf(&pages, &format!("{title} - paint by numbers"))
}

fn text_width_mm_digits(s: &str, size_pt: f32) -> f32 {
    s.len() as f32 * 0.556 * size_pt / PT_PER_MM
}

fn key_page(f: &Frame, title: &str, key: &[KeyEntry]) -> Page {
    let mut p = f.page();
    let m = f.margin.max(10.0);
    let (pw, ph) = f.page;
    p.fill_rgb(30, 30, 30);
    p.text(m, m + 6.0, 16.0, Font::Bold, "Colour key");
    p.fill_rgb(90, 90, 90);
    p.text(m, m + 12.5, 9.0, Font::Regular, title);
    p.text(
        m,
        m + 18.0,
        8.5,
        Font::Regular,
        "Paint a test dab of each paint in the empty box and check it matches the printed colour before you start.",
    );

    let top = m + 26.0;
    let avail_h = ph - m - top - 6.0;
    let col_w = 92.0;
    let max_cols = (((pw - 2.0 * m) / col_w).floor() as usize).max(1);
    let n = key.len().max(1);
    let rows_at_full = ((avail_h / 16.0).floor() as usize).max(1);
    let cols = n.div_ceil(rows_at_full).clamp(1, max_cols);
    let rows = n.div_ceil(cols);
    let row_h = (avail_h / rows as f32).min(16.0);
    let box_h = row_h * 0.72;
    let box_w = (box_h * 1.7).min(22.0);
    let gap = (pw - 2.0 * m - cols as f32 * col_w) / cols as f32;
    let number_pt = (box_h * 0.9 * PT_PER_MM * 0.75).min(13.0);
    let name_pt = (box_h * PT_PER_MM * 0.42).clamp(5.0, 10.0);

    for col in 0..cols {
        let x0 = m + col as f32 * (col_w + gap);
        // column headings
        p.fill_rgb(120, 120, 120);
        p.text(x0 + 11.0, top - 1.5, 7.0, Font::Regular, "Colour");
        p.text(x0 + 13.0 + box_w, top - 1.5, 7.0, Font::Regular, "Your test");
        p.text(x0 + 15.0 + 2.0 * box_w, top - 1.5, 7.0, Font::Regular, "Paint");
    }
    for (i, k) in key.iter().enumerate() {
        let (col, row) = (i / rows, i % rows);
        let x0 = m + col as f32 * (col_w + gap);
        let y0 = top + row as f32 * row_h + (row_h - box_h) / 2.0;
        let num = k.number.to_string();
        p.fill_rgb(40, 40, 40);
        let nw = text_width_mm_digits(&num, number_pt);
        p.text(x0 + 9.0 - nw, y0 + box_h / 2.0 + 0.36 * number_pt / PT_PER_MM, number_pt, Font::Bold, &num);
        let bx = x0 + 11.0;
        p.line_width(0.25).stroke_rgb(80, 80, 80).dash(0.0, 0.0);
        p.fill_rgb(k.color.r, k.color.g, k.color.b);
        p.rect(bx, y0, box_w, box_h, true, true);
        p.rect(bx + box_w + 2.0, y0, box_w, box_h, false, true);
        let tx = bx + 2.0 * box_w + 4.0;
        let name = fit_text(&k.name, name_pt, x0 + col_w - tx - 1.0);
        p.fill_rgb(30, 30, 30);
        p.text(tx, y0 + box_h / 2.0, name_pt, Font::Regular, &name);
        p.fill_rgb(120, 120, 120);
        let pct = k.share * 100.0;
        let info = if pct >= 1.0 {
            format!("{} · {:.0}% of the picture", k.color.to_hex(), pct)
        } else {
            format!("{} · <1% of the picture", k.color.to_hex())
        };
        p.text(tx, y0 + box_h / 2.0 + name_pt / PT_PER_MM + 0.6, (name_pt * 0.75).max(4.5), Font::Regular, &info);
    }
    f.footer(&mut p, title, "4 / 4 · Colour key");
    p
}

/// Shorten `s` with an ellipsis so it fits `max_mm`.
fn fit_text(s: &str, size_pt: f32, max_mm: f32) -> String {
    if text_width_mm(s, size_pt) <= max_mm {
        return s.to_string();
    }
    let mut out: String = s.to_string();
    while !out.is_empty() && text_width_mm(&format!("{out}..."), size_pt) > max_mm {
        out.pop();
    }
    format!("{}...", out.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::Swatch;

    fn three_bands() -> (Segmentation, Palette) {
        // left white, middle red, right blue; a small blue square inside the red band
        let (w, h) = (90u32, 60u32);
        let colors: Vec<u16> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                if (40..46).contains(&x) && (20..26).contains(&y) {
                    2
                } else {
                    (x / 30) as u16
                }
            })
            .collect();
        let mut pal = Palette::new("t");
        pal.swatches = vec![
            Swatch::new("White", Rgb8::new(250, 250, 250)),
            Swatch::new("Red", Rgb8::new(200, 30, 30)),
            Swatch::new("Blue (Phthalo)", Rgb8::new(20, 40, 160)),
        ];
        (Segmentation::from_color_map(w, h, &colors), pal)
    }

    #[test]
    fn key_is_numbered_lightest_first() {
        let (seg, pal) = three_bands();
        let key = color_key(&seg, &pal);
        let names: Vec<_> = key.iter().map(|k| (k.number, k.name.as_str())).collect();
        assert_eq!(names, [(1, "White"), (2, "Red"), (3, "Blue (Phthalo)")]);
        let total: f32 = key.iter().map(|k| k.share).sum();
        assert!((total - 1.0).abs() < 1e-4);
    }

    #[test]
    fn every_region_gets_a_label_inside_it() {
        let (seg, _) = three_bands();
        let labels = label_positions(&seg);
        assert_eq!(labels.len(), seg.region_count());
        for l in &labels {
            let (x, y) = (l.at[0] as u32, l.at[1] as u32);
            assert_eq!(seg.labels[(y * seg.width + x) as usize], l.region);
        }
        // the red band (with a hole) puts its number away from the hole and edges
        let red = labels
            .iter()
            .find(|l| seg.region_color[l.region as usize] == 1)
            .unwrap();
        assert!(red.radius_px >= 7.0, "{red:?}");
        let small = labels
            .iter()
            .find(|l| seg.region_area[l.region as usize] == 36)
            .unwrap();
        assert!((small.at[0] - 43.5).abs() <= 1.0 && (small.at[1] - 23.5).abs() <= 1.0);
    }

    #[test]
    fn packet_has_four_pages_with_numbers_and_key() {
        let (seg, pal) = three_bands();
        let sb = SmoothBoundaries::build(&seg, 1.0);
        let out = OutputSize::default();
        let pdf = packet_pdf(&PacketInput {
            seg: &seg,
            sb: &sb,
            palette: &pal,
            output: &out,
            title: "bands.jpg",
        });
        let s = crate::pdf::readable(&pdf);
        assert!(s.contains("/Count 4"));
        assert!(s.contains("(Colour key) Tj"));
        assert!(s.contains("(Blue \\(Phthalo\\)) Tj"));
        // four regions numbered on page 3: 1, 2, 3 and the small 3
        assert_eq!(s.matches("(3) Tj").count(), 2 + 1, "two blue regions + key row");
    }
}
