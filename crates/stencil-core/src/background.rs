//! Background removal: separate the subject from its background without an ML model.
//!
//! A GrabCut-style loop on a small copy of the image:
//! 1. the image border seeds a colour model of the background (k-means in CIELAB);
//! 2. pixels unlike the background, sharp (in focus) or near the centre start as subject;
//! 3. subject and background colour models are re-fitted from the current split and
//!    every pixel is re-labelled by which model explains it better (scores are blurred,
//!    which keeps the split smooth), a few times;
//! 4. tiny subject specks are dropped and small holes inside the subject are filled.
//!
//! The coarse result is then upsampled, and pixels near the edge are re-decided at full
//! resolution from their own colour, so the outline follows the real edge.

use crate::color::{Lab, Rgb8};
use crate::palette::Palette;
use image::{RgbImage, imageops};

const COARSE: u32 = 200;
const ITERATIONS: usize = 5;
const K: usize = 8;

/// `true` for subject (foreground) pixels, row-major, same size as `img`.
pub fn foreground_mask(img: &RgbImage) -> Vec<bool> {
    let (w, h) = img.dimensions();
    if w < 8 || h < 8 {
        return vec![true; (w * h) as usize];
    }
    let s = (COARSE as f32 / w.max(h) as f32).min(1.0);
    let (cw, ch) = (
        ((w as f32 * s).round() as u32).max(8),
        ((h as f32 * s).round() as u32).max(8),
    );
    let small = imageops::resize(img, cw, ch, imageops::FilterType::Triangle);
    let lab: Vec<[f32; 3]> = small.pixels().map(|p| lab3(Rgb8::new(p[0], p[1], p[2]))).collect();
    let n = lab.len();
    let (cwu, chu) = (cw as usize, ch as usize);

    // --- cues ---------------------------------------------------------------------
    let band = ((cw.min(ch) as f32 * 0.04).round() as usize).max(2);
    let on_border = |i: usize| {
        let (x, y) = (i % cwu, i / cwu);
        x < band || y < band || x >= cwu - band || y >= chu - band
    };
    let detail = detail_map(&lab, cwu, chu);
    let centre: Vec<f32> = (0..n)
        .map(|i| {
            let (x, y) = ((i % cwu) as f32 / cw as f32 - 0.5, (i / cwu) as f32 / ch as f32 - 0.5);
            1.0 - ((x * x + y * y).sqrt() / std::f32::consts::FRAC_1_SQRT_2).min(1.0) * 2.0 // +1 centre .. -1 corners
        })
        .collect();

    // --- initial split --------------------------------------------------------------
    let border: Vec<[f32; 3]> = (0..n).filter(|&i| on_border(i)).map(|i| lab[i]).collect();
    let bg_model = kmeans(&border, K);
    let d_bg: Vec<f32> = lab.iter().map(|p| nearest(&bg_model, *p)).collect();
    let p90 = percentile(&d_bg, 0.9).max(4.0);
    let mut score: Vec<f32> = (0..n)
        .map(|i| (d_bg[i] / p90).min(1.5) + 0.5 * detail[i] + 0.25 * centre[i] - 0.75)
        .collect();
    score = blur(&score, cwu, chu, 1);
    let mut fg: Vec<bool> = (0..n).map(|i| score[i] > 0.0 && !on_border(i)).collect();

    // --- refine -------------------------------------------------------------------
    let mut models = (Vec::new(), Vec::new());
    for _ in 0..ITERATIONS {
        let f: Vec<[f32; 3]> = (0..n).filter(|&i| fg[i]).map(|i| lab[i]).collect();
        let b: Vec<[f32; 3]> = (0..n).filter(|&i| !fg[i] || on_border(i)).map(|i| lab[i]).collect();
        if f.len() < 16 || b.len() < 16 {
            break;
        }
        models = (kmeans(&f, K), kmeans(&b, K));
        let raw: Vec<f32> = (0..n)
            .map(|i| colour_score(&models.0, &models.1, lab[i]) + 0.2 * (detail[i] - 0.25) + 0.15 * centre[i])
            .collect();
        score = blur(&raw, cwu, chu, 3);
        for i in 0..n {
            fg[i] = score[i] > 0.0;
        }
    }

    // --- clean up: drop specks, fill small holes ---------------------------------------
    let comps = components(&fg, cwu, chu);
    let largest = comps
        .iter()
        .filter(|c| c.value)
        .map(|c| c.pixels.len())
        .max()
        .unwrap_or(0);
    for c in &comps {
        let drop_fg = c.value && c.pixels.len() * 10 < largest;
        let fill_bg = !c.value && !c.touches_border && c.pixels.len() * 60 < n;
        if drop_fg || fill_bg {
            for &i in &c.pixels {
                fg[i] = !c.value;
                score[i] = if c.value { -0.5 } else { 0.5 };
            }
        }
    }
    if largest == 0 || models.0.is_empty() {
        return vec![true; (w * h) as usize]; // nothing separable: keep everything
    }

    // --- full resolution: upsample, re-decide the edge band by colour -------------------
    let mut out = vec![false; (w * h) as usize];
    let (sx, sy) = (cw as f32 / w as f32, ch as f32 / h as f32);
    let raw = img.as_raw();
    for y in 0..h {
        let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, ch as f32 - 1.0);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        let y1 = (y0 + 1).min(chu - 1);
        for x in 0..w {
            let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, cw as f32 - 1.0);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            let x1 = (x0 + 1).min(cwu - 1);
            let at = |xx: usize, yy: usize| score[yy * cwu + xx];
            let c = (at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx) * (1.0 - ty)
                + (at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx) * ty;
            let i = (y * w + x) as usize;
            let edge = 1.0 - (c.abs() / 0.3).min(1.0);
            let v = if edge > 0.0 {
                let p = lab3(Rgb8::new(raw[i * 3], raw[i * 3 + 1], raw[i * 3 + 2]));
                c + edge * 0.6 * colour_score(&models.0, &models.1, p)
            } else {
                c
            };
            out[i] = v > 0.0;
        }
    }
    // the edge pass can leave pixel-sized specks and pinholes: tidy them up
    let (wu, hu) = (w as usize, h as usize);
    let comps = components(&out, wu, hu);
    let largest = comps
        .iter()
        .filter(|c| c.value)
        .map(|c| c.pixels.len())
        .max()
        .unwrap_or(0);
    for c in &comps {
        if (c.value && c.pixels.len() * 25 < largest)
            || (!c.value && !c.touches_border && c.pixels.len() * 400 < out.len())
        {
            for &i in &c.pixels {
                out[i] = !c.value;
            }
        }
    }
    out
}

/// Background colours that set the subject off well: paints from `palette` ranked by how
/// far they are from the subject's main colours. Whites and blacks are left out (the UI
/// offers those separately); if the palette has nothing else, a few neutrals are used.
/// `subject` is (colour, share of the subject's area).
pub fn recommend_backgrounds(subject: &[(Rgb8, f32)], palette: &Palette, count: usize) -> Vec<Rgb8> {
    let main: Vec<Lab> = subject
        .iter()
        .filter(|(_, share)| *share >= 0.04)
        .map(|(c, _)| c.to_lab())
        .collect();
    if main.is_empty() {
        return vec![Rgb8::new(255, 255, 255)];
    }
    let not_bw = |c: &Rgb8| {
        let l = c.to_lab();
        l.delta_e2000(Rgb8::new(255, 255, 255).to_lab()) > 10.0 && l.delta_e2000(Rgb8::new(0, 0, 0).to_lab()) > 10.0
    };
    let mut cands: Vec<Rgb8> = palette.swatches.iter().map(|s| s.color).filter(not_bw).collect();
    if cands.len() < count {
        for hex in ["#8C8C8C", "#D9D2C3", "#3E4650", "#B9C7CF"] {
            cands.push(Rgb8::from_hex(hex).unwrap());
        }
    }
    let mut scored: Vec<(f32, Rgb8)> = cands
        .into_iter()
        .map(|c| {
            let l = c.to_lab();
            let min = main.iter().map(|m| m.delta_e2000(l)).fold(f32::MAX, f32::min);
            // a little preference for calmer (less saturated) backdrops
            let chroma = (l.a * l.a + l.b * l.b).sqrt();
            (min - 0.08 * chroma, c)
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut out: Vec<Rgb8> = Vec::new();
    for (_, c) in scored {
        if out.iter().all(|o| o.to_lab().delta_e2000(c.to_lab()) > 12.0) {
            out.push(c);
        }
        if out.len() == count {
            break;
        }
    }
    out
}

fn lab3(c: Rgb8) -> [f32; 3] {
    let l = c.to_lab();
    [l.l, l.a, l.b]
}

fn d2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

fn nearest(model: &[[f32; 3]], p: [f32; 3]) -> f32 {
    model.iter().map(|&c| d2(c, p)).fold(f32::MAX, f32::min).sqrt()
}

/// In [-1, 1]: positive when the subject model explains `p` better than the background's.
fn colour_score(fg: &[[f32; 3]], bg: &[[f32; 3]], p: [f32; 3]) -> f32 {
    let (df, db) = (nearest(fg, p), nearest(bg, p));
    (db - df) / (db + df + 2.0)
}

fn percentile(v: &[f32], q: f32) -> f32 {
    let mut s: Vec<f32> = v.to_vec();
    s.sort_by(f32::total_cmp);
    s.get(((s.len() as f32 * q) as usize).min(s.len().saturating_sub(1)))
        .copied()
        .unwrap_or(0.0)
}

/// Lloyd's k-means with farthest-point seeding (deterministic).
fn kmeans(pts: &[[f32; 3]], k: usize) -> Vec<[f32; 3]> {
    if pts.is_empty() {
        return Vec::new();
    }
    let step = (pts.len() / 4000).max(1);
    let sample: Vec<[f32; 3]> = pts.iter().step_by(step).copied().collect();
    let mut mean = [0f32; 3];
    for p in &sample {
        for j in 0..3 {
            mean[j] += p[j] / sample.len() as f32;
        }
    }
    let mut centres = vec![
        *sample
            .iter()
            .max_by(|a, b| d2(**a, mean).total_cmp(&d2(**b, mean)))
            .unwrap(),
    ];
    while centres.len() < k.min(sample.len()) {
        let far = sample
            .iter()
            .max_by(|a, b| nearest(&centres, **a).total_cmp(&nearest(&centres, **b)))
            .unwrap();
        if nearest(&centres, *far) < 1.0 {
            break;
        }
        centres.push(*far);
    }
    for _ in 0..8 {
        let mut acc = vec![([0f32; 3], 0f32); centres.len()];
        for p in &sample {
            let j = (0..centres.len())
                .min_by(|&a, &b| d2(centres[a], *p).total_cmp(&d2(centres[b], *p)))
                .unwrap();
            for t in 0..3 {
                acc[j].0[t] += p[t];
            }
            acc[j].1 += 1.0;
        }
        for (c, (s, n)) in centres.iter_mut().zip(&acc) {
            if *n > 0.0 {
                *c = [s[0] / n, s[1] / n, s[2] / n];
            }
        }
    }
    centres
}

/// Local contrast of L, in [0, 1).
fn detail_map(lab: &[[f32; 3]], w: usize, h: usize) -> Vec<f32> {
    let mut g = vec![0f32; w * h];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            let gx = lab[i + 1][0] - lab[i - 1][0];
            let gy = lab[i + w][0] - lab[i - w][0];
            g[i] = (gx * gx + gy * gy).sqrt();
        }
    }
    // Absolute scale (L units across two pixels): sensor noise and smooth bokeh stay
    // near 0, in-focus texture and edges approach 1.
    let g = blur(&g, w, h, 2);
    g.iter().map(|v| v / (v + 12.0)).collect()
}

/// Separable box blur, radius `r`.
fn blur(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let pass = |src: &[f32], horiz: bool| -> Vec<f32> {
        let mut out = vec![0f32; src.len()];
        for y in 0..h {
            for x in 0..w {
                let (mut s, mut n) = (0f32, 0f32);
                for d in -(r as i64)..=(r as i64) {
                    let (xx, yy) = if horiz {
                        (x as i64 + d, y as i64)
                    } else {
                        (x as i64, y as i64 + d)
                    };
                    if xx >= 0 && yy >= 0 && (xx as usize) < w && (yy as usize) < h {
                        s += src[yy as usize * w + xx as usize];
                        n += 1.0;
                    }
                }
                out[y * w + x] = s / n;
            }
        }
        out
    };
    pass(&pass(v, true), false)
}

struct Comp {
    value: bool,
    touches_border: bool,
    pixels: Vec<usize>,
}

fn components(m: &[bool], w: usize, h: usize) -> Vec<Comp> {
    let mut seen = vec![false; m.len()];
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for s in 0..m.len() {
        if seen[s] {
            continue;
        }
        let v = m[s];
        let mut c = Comp {
            value: v,
            touches_border: false,
            pixels: Vec::new(),
        };
        seen[s] = true;
        stack.push(s);
        while let Some(i) = stack.pop() {
            c.pixels.push(i);
            let (x, y) = (i % w, i / w);
            if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                c.touches_border = true;
            }
            let mut visit = |j: usize| {
                if !seen[j] && m[j] == v {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_disc_on_a_plain_background() {
        let img = RgbImage::from_fn(300, 200, |x, y| {
            let (dx, dy) = (x as f32 - 150.0, y as f32 - 100.0);
            if dx * dx + dy * dy < 60.0 * 60.0 {
                image::Rgb([200, 40, 30])
            } else {
                // slightly noisy greenish background
                image::Rgb([60 + ((x * 7 + y * 3) % 9) as u8, 120, 70])
            }
        });
        let m = foreground_mask(&img);
        let mut wrong = 0;
        for y in 0..200u32 {
            for x in 0..300u32 {
                let (dx, dy) = (x as f32 - 150.0, y as f32 - 100.0);
                let inside = dx * dx + dy * dy < 60.0 * 60.0;
                if m[(y * 300 + x) as usize] != inside {
                    wrong += 1;
                }
            }
        }
        assert!(wrong < 300 * 200 / 100, "{wrong} wrong pixels");
    }

    #[test]
    fn recommends_contrasting_colours() {
        let pal = Palette::basic_acrylic_12();
        let rec = recommend_backgrounds(&[(Rgb8::new(250, 250, 250), 1.0)], &pal, 3);
        assert_eq!(rec.len(), 3);
        assert!(rec[0].to_lab().l < 60.0, "white subject should get a darker backdrop");
        assert!(
            rec.iter().all(|c| c.to_lab().l > 8.0 && c.to_lab().l < 95.0),
            "no pure black/white"
        );
    }
}
