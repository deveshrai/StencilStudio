//! Reading a filled-in calibration sheet back from a photo or scan.
//!
//! 1. Find the four corner markers: dark, square blobs (judged by their second
//!    moments, so any rotation works). Three are solid; the hollow one marks the
//!    bottom-right corner, which fixes the orientation even if the photo is upside down.
//! 2. Map page millimetres to image pixels with the homography through the markers.
//!    A4 and US Letter are both tried; the better-aligned one wins.
//! 3. Correct colour with the gray reference strip: paper white, black and five grays
//!    give a per-channel tone curve that removes the colour cast and exposure.
//! 4. Read each swatch box from its central area (median colour); boxes that are still
//!    paper-white are reported as empty.

use crate::calibration::{self, RectMm, SWATCH_COUNT, SheetLayout};
use crate::color::Rgb8;
use crate::paper::PaperSize;
use image::RgbImage;

/// What was read from the sheet.
#[derive(Debug, Clone)]
pub struct ScanReading {
    /// Paper the sheet was printed on ("A4" or "US Letter").
    pub paper: String,
    /// One entry per swatch box (reading order). `None` = left empty.
    pub colors: Vec<Option<Rgb8>>,
    /// Measured paper white before correction (a strong cast means poor light).
    pub paper_white: Rgb8,
}

impl ScanReading {
    pub fn filled(&self) -> usize {
        self.colors.iter().filter(|c| c.is_some()).count()
    }
}

/// Load a scan or photo of the sheet: JPG/PNG/WebP, or a PDF from a scanner app
/// (its largest embedded picture). Transparency is flattened onto white paper.
pub fn load_scan(path: &std::path::Path) -> Result<RgbImage, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.starts_with(b"%PDF") {
        return crate::pdf_image::largest_image(&bytes);
    }
    let img = image::load_from_memory(&bytes).map_err(|e| format!("Couldn't read the image: {e}"))?;
    let rgba = img.to_rgba8();
    Ok(RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let p = rgba.get_pixel(x, y);
        let a = p[3] as f32 / 255.0;
        image::Rgb([0, 1, 2].map(|k| (p[k] as f32 * a + 255.0 * (1.0 - a)).round() as u8))
    }))
}

/// Read a photographed / scanned calibration sheet.
pub fn read_sheet(img: &RgbImage) -> Result<ScanReading, String> {
    // detection works on a moderate size; sampling uses the full image
    let (w0, h0) = img.dimensions();
    let s = (1600.0 / w0.max(h0) as f32).min(1.0);
    let small = if s < 1.0 {
        image::imageops::resize(
            img,
            (w0 as f32 * s).round() as u32,
            (h0 as f32 * s).round() as u32,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img.clone()
    };
    let corners = find_markers(&small)?;
    let corners = corners.map(|(x, y)| (x / s, y / s));

    let papers = [
        PaperSize::new("A4", 210.0, 297.0),
        PaperSize::new("US Letter", 215.9, 279.4),
    ];
    let mut best: Option<(f32, ScanReading)> = None;
    for paper in &papers {
        let l = calibration::layout(paper);
        let src: [(f32, f32); 4] = [0, 1, 3, 2].map(|i| {
            let r = l.fiducials[i];
            (r.x + r.w / 2.0, r.y + r.h / 2.0)
        });
        let Some(hm) = Homography::from_points(&src, &corners) else {
            continue;
        };
        let (reading, misfit) = read_with(img, &l, &hm, &paper.name);
        if best.as_ref().is_none_or(|b| misfit < b.0) {
            best = Some((misfit, reading));
        }
    }
    best.map(|b| b.1)
        .ok_or_else(|| "Couldn't line up the sheet. Make sure all four corner markers are visible.".into())
}

/// Sample everything for one paper layout. Returns the reading and a misfit score
/// (how much the patches vary inside: lower = better aligned).
fn read_with(img: &RgbImage, l: &SheetLayout, hm: &Homography, paper: &str) -> (ScanReading, f32) {
    let mut misfit = 0.0;
    let mut grays: Vec<([f32; 3], f32)> = Vec::new();
    for (r, level) in &l.gray_strip {
        let (c, spread) = sample_rect(img, hm, r, 0.5);
        misfit += spread;
        grays.push((c, *level as f32));
    }
    let curve = ToneCurve::from_grays(&grays);
    let paper_white = {
        let c = grays.first().map(|g| g.0).unwrap_or([255.0; 3]);
        Rgb8::new(c[0] as u8, c[1] as u8, c[2] as u8)
    };
    let mut colors = Vec::with_capacity(SWATCH_COUNT);
    for b in &l.boxes {
        let (c, spread) = sample_rect(img, hm, b, 0.55);
        misfit += spread * 0.25;
        let c = curve.apply(c);
        let rgb = Rgb8::new(c[0], c[1], c[2]);
        let white = Rgb8::new(255, 255, 255);
        let empty = rgb.to_lab().delta_e2000(white.to_lab()) < 7.0;
        colors.push((!empty).then_some(rgb));
    }
    (
        ScanReading {
            paper: paper.to_string(),
            colors,
            paper_white,
        },
        misfit,
    )
}

/// Median colour (and mean channel spread) of the central `frac` of a page rectangle.
fn sample_rect(img: &RgbImage, hm: &Homography, r: &RectMm, frac: f32) -> ([f32; 3], f32) {
    let (cx, cy) = r.center();
    let (hw, hh) = (r.w * frac / 2.0, r.h * frac / 2.0);
    let n = 9;
    let mut ch: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for j in 0..n {
        for i in 0..n {
            let x = cx - hw + 2.0 * hw * (i as f32 + 0.5) / n as f32;
            let y = cy - hh + 2.0 * hh * (j as f32 + 0.5) / n as f32;
            let (px, py) = hm.map(x, y);
            if let Some(p) = bilinear(img, px, py) {
                for k in 0..3 {
                    ch[k].push(p[k]);
                }
            }
        }
    }
    let mut out = [255.0; 3];
    let mut spread = 0.0;
    for k in 0..3 {
        if ch[k].is_empty() {
            return ([255.0; 3], 1e3);
        }
        ch[k].sort_by(f32::total_cmp);
        let m = ch[k].len();
        out[k] = ch[k][m / 2];
        spread += ch[k][m * 9 / 10] - ch[k][m / 10];
    }
    (out, spread / 3.0)
}

fn bilinear(img: &RgbImage, x: f32, y: f32) -> Option<[f32; 3]> {
    let (w, h) = img.dimensions();
    if x < 0.0 || y < 0.0 || x > w as f32 - 1.0 || y > h as f32 - 1.0 {
        return None;
    }
    let (x0, y0) = (x.floor() as u32, y.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (x - x0 as f32, y - y0 as f32);
    let p = |x, y| img.get_pixel(x, y).0;
    let mut o = [0.0; 3];
    for (k, v) in o.iter_mut().enumerate() {
        let a = p(x0, y0)[k] as f32 * (1.0 - tx) + p(x1, y0)[k] as f32 * tx;
        let b = p(x0, y1)[k] as f32 * (1.0 - tx) + p(x1, y1)[k] as f32 * tx;
        *v = a * (1.0 - ty) + b * ty;
    }
    Some(o)
}

/// Per-channel piecewise-linear map from measured values to the printed gray levels.
struct ToneCurve {
    pts: [Vec<(f32, f32)>; 3],
}

impl ToneCurve {
    fn from_grays(g: &[([f32; 3], f32)]) -> Self {
        let pts = [0, 1, 2].map(|k| {
            let mut v: Vec<(f32, f32)> = g.iter().map(|(m, t)| (m[k], *t)).collect();
            v.sort_by(|a, b| a.0.total_cmp(&b.0));
            // keep it monotonic: drop points that would bend the curve backwards
            let mut out: Vec<(f32, f32)> = Vec::new();
            for p in v {
                if out.last().is_none_or(|l| p.0 > l.0 + 0.5 && p.1 >= l.1) {
                    out.push(p);
                }
            }
            if out.len() < 2 {
                out = vec![(0.0, 0.0), (255.0, 255.0)];
            }
            out
        });
        Self { pts }
    }

    fn apply(&self, c: [f32; 3]) -> [u8; 3] {
        [0, 1, 2].map(|k| {
            let p = &self.pts[k];
            let x = c[k];
            let i = p
                .iter()
                .position(|q| q.0 >= x)
                .unwrap_or(p.len() - 1)
                .clamp(1, p.len() - 1);
            let (a, b) = (p[i - 1], p[i]);
            let t = (x - a.0) / (b.0 - a.0).max(1e-3);
            (a.1 + t * (b.1 - a.1)).round().clamp(0.0, 255.0) as u8
        })
    }
}

/// Projective map page-mm → image-px.
struct Homography([f32; 9]);

impl Homography {
    /// From 4 point pairs (DLT, solved by Gaussian elimination).
    fn from_points(src: &[(f32, f32); 4], dst: &[(f32, f32); 4]) -> Option<Self> {
        let mut a = [[0f64; 9]; 8];
        for i in 0..4 {
            let (x, y) = (src[i].0 as f64, src[i].1 as f64);
            let (u, v) = (dst[i].0 as f64, dst[i].1 as f64);
            a[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
            a[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
        }
        for c in 0..8 {
            let p = (c..8).max_by(|&i, &j| a[i][c].abs().total_cmp(&a[j][c].abs()))?;
            if a[p][c].abs() < 1e-12 {
                return None;
            }
            a.swap(c, p);
            for r in 0..8 {
                if r != c {
                    let f = a[r][c] / a[c][c];
                    for k in c..9 {
                        a[r][k] -= f * a[c][k];
                    }
                }
            }
        }
        let mut hm = [0f32; 9];
        for i in 0..8 {
            hm[i] = (a[i][8] / a[i][i]) as f32;
        }
        hm[8] = 1.0;
        Some(Self(hm))
    }

    fn map(&self, x: f32, y: f32) -> (f32, f32) {
        let h = &self.0;
        let d = h[6] * x + h[7] * y + h[8];
        ((h[0] * x + h[1] * y + h[2]) / d, (h[3] * x + h[4] * y + h[5]) / d)
    }
}

struct Blob {
    cx: f32,
    cy: f32,
    area: f32,
    /// area / (side estimated from second moments)²: ≈1 solid square, ≈0.6 hollow.
    solidity: f32,
}

/// Image positions of the marker centres in page order: top-left, top-right,
/// bottom-right (the hollow one), bottom-left.
fn find_markers(img: &RgbImage) -> Result<[(f32, f32); 4], String> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let lum: Vec<f32> = img
        .pixels()
        .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
        .collect();
    // dark relative to the local paper brightness (copes with uneven light)
    let local = box_mean(&lum, w, h, (w.max(h) / 14).max(4));
    let dark: Vec<bool> = lum
        .iter()
        .zip(&local)
        .map(|(&l, &m)| l < 0.55 * m && l < 150.0)
        .collect();

    let mut blobs = Vec::new();
    let mut seen = vec![false; w * h];
    let mut stack = Vec::new();
    let min_area = ((w.max(h) as f32 / 120.0).powi(2)) as usize;
    for s in 0..w * h {
        if !dark[s] || seen[s] {
            continue;
        }
        seen[s] = true;
        stack.push(s);
        let (mut n, mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
        while let Some(i) = stack.pop() {
            let (x, y) = ((i % w) as f64, (i / w) as f64);
            n += 1.0;
            sx += x;
            sy += y;
            sxx += x * x;
            syy += y * y;
            sxy += x * y;
            let (xi, yi) = (i % w, i / w);
            for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let (nx, ny) = (xi as i32 + dx, yi as i32 + dy);
                if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
                    let j = ny as usize * w + nx as usize;
                    if dark[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        if (n as usize) < min_area || n as usize > w * h / 40 {
            continue;
        }
        let (mx, my) = (sx / n, sy / n);
        let (vxx, vyy, vxy) = (sxx / n - mx * mx, syy / n - my * my, sxy / n - mx * my);
        let tr = vxx + vyy;
        let det = vxx * vyy - vxy * vxy;
        let disc = (tr * tr / 4.0 - det).max(0.0).sqrt();
        let (l1, l2) = (tr / 2.0 + disc, tr / 2.0 - disc);
        if l2 <= 0.0 || l1 / l2 > 1.8 {
            continue; // elongated: text line, gray strip, swatch box
        }
        let side2 = 12.0 * tr / 2.0;
        blobs.push(Blob {
            cx: mx as f32,
            cy: my as f32,
            area: n as f32,
            solidity: (n / side2) as f32,
        });
    }
    blobs.retain(|b| b.solidity > 0.45 && b.solidity < 1.25);
    if blobs.len() < 4 {
        return Err(
            "Couldn't find the four black corner markers. Photograph the whole sheet, flat and well lit.".into(),
        );
    }
    // markers are the largest square blobs; keep those of similar size
    blobs.sort_by(|a, b| b.area.total_cmp(&a.area));
    let ref_area = blobs[..4.min(blobs.len())]
        .iter()
        .map(|b| b.area)
        .fold(f32::MAX, f32::min);
    let cands: Vec<&Blob> = blobs.iter().filter(|b| b.area > ref_area * 0.3).take(12).collect();
    // the four that span the largest quadrilateral
    let mut best: Option<(f32, [usize; 4])> = None;
    let n = cands.len();
    for a in 0..n {
        for b in a + 1..n {
            for c in b + 1..n {
                for d in c + 1..n {
                    let ids = [a, b, c, d];
                    let areas: Vec<f32> = ids.iter().map(|&i| cands[i].area).collect();
                    let (amin, amax) = areas.iter().fold((f32::MAX, 0f32), |(l, h), &v| (l.min(v), h.max(v)));
                    if amax > amin * 4.0 {
                        continue;
                    }
                    let quad = order_clockwise(ids.map(|i| (cands[i].cx, cands[i].cy)));
                    let area = polygon_area(&quad);
                    if best.as_ref().is_none_or(|bst| area > bst.0) {
                        best = Some((area, ids));
                    }
                }
            }
        }
    }
    let (_, ids) = best.ok_or("Couldn't find the four corner markers.")?;
    let mut pts: Vec<(f32, f32, f32)> = ids
        .iter()
        .map(|&i| (cands[i].cx, cands[i].cy, cands[i].solidity))
        .collect();
    // clockwise in image coordinates (y down)
    let (mx, my) = (
        pts.iter().map(|p| p.0).sum::<f32>() / 4.0,
        pts.iter().map(|p| p.1).sum::<f32>() / 4.0,
    );
    pts.sort_by(|a, b| (a.1 - my).atan2(a.0 - mx).total_cmp(&(b.1 - my).atan2(b.0 - mx)));
    let hollow = (0..4).min_by(|&a, &b| pts[a].2.total_cmp(&pts[b].2)).unwrap();
    let solid_min = (0..4)
        .filter(|&i| i != hollow)
        .map(|i| pts[i].2)
        .fold(f32::MAX, f32::min);
    if solid_min - pts[hollow].2 < 0.12 {
        return Err(
            "Found the corners but not the hollow bottom-right marker. Is the whole sheet in the picture?".into(),
        );
    }
    // page order TL, TR, BR, BL is clockwise with the hollow one third
    let start = (hollow + 2) % 4;
    let o: [(f32, f32); 4] = [0, 1, 2, 3].map(|k| {
        let p = pts[(start + k) % 4];
        (p.0, p.1)
    });
    Ok(o)
}

fn order_clockwise(p: [(f32, f32); 4]) -> [(f32, f32); 4] {
    let (mx, my) = (
        p.iter().map(|q| q.0).sum::<f32>() / 4.0,
        p.iter().map(|q| q.1).sum::<f32>() / 4.0,
    );
    let mut v = p.to_vec();
    v.sort_by(|a, b| (a.1 - my).atan2(a.0 - mx).total_cmp(&(b.1 - my).atan2(b.0 - mx)));
    [v[0], v[1], v[2], v[3]]
}

fn polygon_area(p: &[(f32, f32)]) -> f32 {
    let n = p.len();
    (0..n)
        .map(|i| p[i].0 * p[(i + 1) % n].1 - p[(i + 1) % n].0 * p[i].1)
        .sum::<f32>()
        .abs()
        / 2.0
}

fn box_mean(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    // summed-area table
    let mut sat = vec![0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0f64;
        for x in 0..w {
            row += v[y * w + x] as f64;
            sat[(y + 1) * (w + 1) + x + 1] = sat[y * (w + 1) + x + 1] + row;
        }
    }
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let s = sat[y1 * (w + 1) + x1] - sat[y0 * (w + 1) + x1] - sat[y1 * (w + 1) + x0] + sat[y0 * (w + 1) + x0];
            out[y * w + x] = (s / ((y1 - y0) * (x1 - x0)) as f64) as f32;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Draw the sheet at `ppm` px/mm with `paints` in the first boxes.
    fn synthetic_sheet(paper: &PaperSize, ppm: f32, paints: &[Rgb8]) -> RgbImage {
        let l = calibration::layout(paper);
        let (w, h) = ((l.page_mm.0 * ppm) as u32, (l.page_mm.1 * ppm) as u32);
        let mut img = RgbImage::from_pixel(w, h, image::Rgb([255, 255, 255]));
        let mut fill = |r: &RectMm, c: [u8; 3]| {
            for y in (r.y * ppm) as u32..((r.y + r.h) * ppm) as u32 {
                for x in (r.x * ppm) as u32..((r.x + r.w) * ppm) as u32 {
                    img.put_pixel(x, y, image::Rgb(c));
                }
            }
        };
        for (i, f) in l.fiducials.iter().enumerate() {
            fill(f, [0, 0, 0]);
            if i == 3 {
                fill(
                    &RectMm {
                        x: f.x + 2.5,
                        y: f.y + 2.5,
                        w: f.w - 5.0,
                        h: f.h - 5.0,
                    },
                    [255, 255, 255],
                );
            }
        }
        for (r, g) in &l.gray_strip {
            fill(r, [*g, *g, *g]);
        }
        for (b, c) in l.boxes.iter().zip(paints) {
            fill(b, [c.r, c.g, c.b]);
        }
        img
    }

    /// Photograph it: rotate 180°, perspective, warm light cast, darker exposure.
    fn photograph(sheet: &RgbImage) -> RgbImage {
        let (w, h) = sheet.dimensions();
        let (ow, oh) = (1400u32, 1100u32);
        // where the page corners land in the photo (rotated 180°, keystoned)
        let page = [(0.0, 0.0), (w as f32, 0.0), (w as f32, h as f32), (0.0, h as f32)];
        let photo = [(1150.0, 1000.0), (330.0, 1040.0), (380.0, 60.0), (1080.0, 110.0)];
        let inv = Homography::from_points(&photo, &page).unwrap();
        RgbImage::from_fn(ow, oh, |x, y| {
            let (sx, sy) = inv.map(x as f32, y as f32);
            let c = bilinear(sheet, sx, sy).unwrap_or([60.0, 50.0, 40.0]);
            let cast = [0.92, 0.84, 0.66];
            image::Rgb([0, 1, 2].map(|k| (255.0 * (c[k] / 255.0).powf(1.15) * cast[k]) as u8))
        })
    }

    #[test]
    fn reads_a_rotated_warped_colour_cast_photo() {
        let paints = [
            Rgb8::new(200, 30, 35),
            Rgb8::new(30, 60, 170),
            Rgb8::new(240, 200, 20),
            Rgb8::new(40, 120, 60),
            Rgb8::new(120, 70, 40),
            Rgb8::new(30, 30, 30),
        ];
        let paper = PaperSize::new("A4", 210.0, 297.0);
        let photo = photograph(&synthetic_sheet(&paper, 6.0, &paints));
        let r = read_sheet(&photo).expect("read");
        assert_eq!(r.paper, "A4");
        assert_eq!(r.filled(), paints.len(), "{:?}", r.colors);
        for (got, want) in r.colors.iter().zip(&paints) {
            let got = got.expect("filled");
            let de = got.to_lab().delta_e2000(want.to_lab());
            assert!(de < 4.0, "{got:?} vs {want:?}: ΔE {de}");
        }
        assert!(r.colors[paints.len()..].iter().all(|c| c.is_none()));
    }

    #[test]
    fn letter_sheet_is_recognised() {
        let paper = PaperSize::new("US Letter", 215.9, 279.4);
        let img = synthetic_sheet(&paper, 5.0, &[Rgb8::new(200, 30, 35)]);
        let r = read_sheet(&img).expect("read");
        assert_eq!(r.paper, "US Letter");
        assert_eq!(r.filled(), 1);
    }

    #[test]
    fn photo_without_markers_is_an_error() {
        let img = RgbImage::from_pixel(800, 600, image::Rgb([200, 180, 160]));
        assert!(read_sheet(&img).is_err());
    }
}
