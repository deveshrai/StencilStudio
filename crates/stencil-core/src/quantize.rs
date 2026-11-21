//! Perceptual color quantization: choose which of the user's physical colors
//! to use, and map every pixel to one of them.

use crate::color::{DistanceMetric, Lab, Rgb8};
use crate::palette::{Palette, Swatch};
use image::RgbImage;
use rayon::prelude::*;

const BITS: u32 = 5;
const LEVELS: usize = 1 << BITS;
const SHIFT: u32 = 8 - BITS;
const BUCKETS: usize = LEVELS * LEVELS * LEVELS;

#[inline]
fn bucket_of(p: [u8; 3]) -> usize {
    ((p[0] as usize >> SHIFT) << (2 * BITS)) | ((p[1] as usize >> SHIFT) << BITS) | (p[2] as usize >> SHIFT)
}

/// A weighted color histogram of an image, bucketed at 5 bits per channel.
pub struct Histogram {
    /// Bucket index -> index into `samples` (u32::MAX if empty).
    bucket_to_sample: Vec<u32>,
    /// Mean color of each non-empty bucket and its pixel count.
    pub samples: Vec<(Lab, f32)>,
}

impl Histogram {
    pub fn build(img: &RgbImage) -> Self {
        Self::build_weighted(img, None)
    }

    /// Like [`build`](Self::build), but each pixel counts with its own weight (e.g. from
    /// [`detail_weights`]). Bucket colors are still plain averages.
    pub fn build_weighted(img: &RgbImage, weights: Option<&[f32]>) -> Self {
        let mut acc = vec![(0u64, 0u64, 0u64, 0u32, 0f32); BUCKETS];
        for (i, p) in img.pixels().enumerate() {
            let e = &mut acc[bucket_of(p.0)];
            e.0 += p.0[0] as u64;
            e.1 += p.0[1] as u64;
            e.2 += p.0[2] as u64;
            e.3 += 1;
            e.4 += weights.map_or(1.0, |w| w[i]);
        }
        let mut bucket_to_sample = vec![u32::MAX; BUCKETS];
        let mut samples = Vec::new();
        for (i, &(r, g, b, n, wsum)) in acc.iter().enumerate() {
            if n > 0 {
                let n64 = n as u64;
                let mean = Rgb8::new((r / n64) as u8, (g / n64) as u8, (b / n64) as u8);
                bucket_to_sample[i] = samples.len() as u32;
                samples.push((mean.to_lab(), wsum));
            }
        }
        Self {
            bucket_to_sample,
            samples,
        }
    }

    fn sample_of(&self, p: [u8; 3]) -> usize {
        self.bucket_to_sample[bucket_of(p)] as usize
    }
}

/// Per-pixel importance from local detail: sharp, in-focus areas (usually the subject)
/// weigh up to ~5x more than smooth or out-of-focus areas such as bokeh backgrounds.
pub fn detail_weights(img: &RgbImage) -> Vec<f32> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w < 3 || h < 3 {
        return vec![1.0; w * h];
    }
    let lum: Vec<f32> = img
        .pixels()
        .map(|p| 0.299 * p.0[0] as f32 + 0.587 * p.0[1] as f32 + 0.114 * p.0[2] as f32)
        .collect();
    // gradient magnitude (central differences)
    let mut g = vec![0f32; w * h];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            let gx = lum[i + 1] - lum[i - 1];
            let gy = lum[i + w] - lum[i - w];
            g[i] = (gx * gx + gy * gy).sqrt();
        }
    }
    // spread detail over its neighbourhood (separable box blur) so whole objects count
    let r = (w.max(h) / 150).max(2);
    let blur = |src: &[f32], horizontal: bool| -> Vec<f32> {
        let mut out = vec![0f32; w * h];
        let (n, m) = if horizontal { (h, w) } else { (w, h) };
        for a in 0..n {
            let at = |b: usize| if horizontal { a * w + b } else { b * w + a };
            let mut sum = 0f32;
            let mut cnt = 0f32;
            for b in 0..m.min(r + 1) {
                sum += src[at(b)];
                cnt += 1.0;
            }
            for b in 0..m {
                out[at(b)] = sum / cnt;
                if b + r + 1 < m {
                    sum += src[at(b + r + 1)];
                    cnt += 1.0;
                }
                if b >= r {
                    sum -= src[at(b - r)];
                    cnt -= 1.0;
                }
            }
        }
        out
    };
    let g = blur(&blur(&g, true), false);
    let mut sorted: Vec<f32> = g.iter().step_by(7).copied().collect();
    sorted.sort_by(f32::total_cmp);
    let p90 = sorted.get(sorted.len() * 9 / 10).copied().unwrap_or(1.0).max(1e-3);
    g.iter().map(|&v| 0.25 + (v / p90).min(1.0)).collect()
}

/// Choose the `k` palette colors that best reproduce the image, by minimizing the
/// pixel-weighted perceptual error (greedy selection + swap refinement).
/// Returns indices into `palette.swatches`, sorted ascending.
pub fn select_palette_subset(hist: &Histogram, palette: &Palette, k: usize, metric: DistanceMetric) -> Vec<usize> {
    let n = palette.len();
    if n == 0 {
        return Vec::new();
    }
    let k = k.clamp(1, n);
    let pal_lab: Vec<Lab> = palette.swatches.iter().map(Swatch::lab).collect();
    // dist[p][s]: distance from palette color p to histogram sample s.
    let dist: Vec<Vec<f32>> = pal_lab
        .par_iter()
        .map(|&pl| hist.samples.iter().map(|&(sl, _)| metric.distance(pl, sl)).collect())
        .collect();
    // Saliency: colorful pixels matter more to viewers than dull ones.
    let weights: Vec<f32> = hist
        .samples
        .iter()
        .map(|(lab, w)| w * (1.0 + (lab.a * lab.a + lab.b * lab.b).sqrt() / 25.0))
        .collect();

    let cost_of = |sel: &[usize]| -> f64 {
        (0..weights.len())
            .into_par_iter()
            .map(|s| {
                let m = sel.iter().map(|&p| dist[p][s]).fold(f32::INFINITY, f32::min);
                (m * weights[s]) as f64
            })
            .sum()
    };

    // Greedy forward selection.
    let mut selected: Vec<usize> = Vec::with_capacity(k);
    let mut best_min = vec![f32::INFINITY; weights.len()];
    while selected.len() < k {
        let (best_p, _) = (0..n)
            .filter(|p| !selected.contains(p))
            .map(|p| {
                let c: f64 = best_min
                    .iter()
                    .zip(&dist[p])
                    .zip(&weights)
                    .map(|((&m, &d), &w)| (m.min(d) * w) as f64)
                    .sum();
                (p, c)
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        selected.push(best_p);
        for (m, &d) in best_min.iter_mut().zip(&dist[best_p]) {
            *m = m.min(d);
        }
    }

    // Swap refinement (bounded passes).
    if k < n {
        let mut cost = cost_of(&selected);
        for _ in 0..3 {
            let mut improved = false;
            for i in 0..k {
                for cand in 0..n {
                    if selected.contains(&cand) {
                        continue;
                    }
                    let old = selected[i];
                    selected[i] = cand;
                    let c = cost_of(&selected);
                    if c + 1e-6 < cost {
                        cost = c;
                        improved = true;
                    } else {
                        selected[i] = old;
                    }
                }
            }
            if !improved {
                break;
            }
        }
    }
    selected.sort_unstable();
    selected
}

/// Derive a palette of `k` colors from the image itself (weighted k-means++ in CIELAB).
/// Used when the user has not supplied their own colors yet.
pub fn auto_palette(hist: &Histogram, k: usize) -> Palette {
    let samples = &hist.samples;
    let k = k.clamp(1, samples.len().max(1));
    let mut rng = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 11) as f64 / (1u64 << 53) as f64
    };
    // k-means++ seeding, starting from the heaviest sample.
    let first = samples
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.1.total_cmp(&b.1.1))
        .map(|x| x.0)
        .unwrap_or(0);
    let mut centers = vec![samples[first].0];
    let mut d2: Vec<f32> = samples.iter().map(|s| s.0.dist2(centers[0])).collect();
    while centers.len() < k {
        let total: f64 = d2.iter().zip(samples).map(|(&d, s)| (d * s.1) as f64).sum();
        if total <= 0.0 {
            break;
        }
        let mut target = next() * total;
        let mut pick = samples.len() - 1;
        for (i, (&d, s)) in d2.iter().zip(samples).enumerate() {
            target -= (d * s.1) as f64;
            if target <= 0.0 {
                pick = i;
                break;
            }
        }
        let c = samples[pick].0;
        centers.push(c);
        for (d, s) in d2.iter_mut().zip(samples) {
            *d = d.min(s.0.dist2(c));
        }
    }
    // Lloyd iterations.
    for _ in 0..20 {
        let mut sums = vec![(0f64, 0f64, 0f64, 0f64); centers.len()];
        for &(lab, w) in samples {
            let ci = nearest(&centers, lab);
            let e = &mut sums[ci];
            e.0 += (lab.l * w) as f64;
            e.1 += (lab.a * w) as f64;
            e.2 += (lab.b * w) as f64;
            e.3 += w as f64;
        }
        let mut moved = 0f32;
        for (c, s) in centers.iter_mut().zip(&sums) {
            if s.3 > 0.0 {
                let n = Lab::new((s.0 / s.3) as f32, (s.1 / s.3) as f32, (s.2 / s.3) as f32);
                moved = moved.max(c.dist2(n));
                *c = n;
            }
        }
        if moved < 0.01 {
            break;
        }
    }
    centers.sort_by(|a, b| b.l.total_cmp(&a.l));
    Palette {
        name: "Auto (from image)".into(),
        swatches: centers
            .iter()
            .enumerate()
            .map(|(i, c)| Swatch::new(format!("Color {}", i + 1), c.to_rgb()))
            .collect(),
    }
}

fn nearest(centers: &[Lab], c: Lab) -> usize {
    centers
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.dist2(c).total_cmp(&b.1.dist2(c)))
        .map(|x| x.0)
        .unwrap_or(0)
}

/// Map each pixel to the nearest of `selected` palette colors.
/// Returns, per pixel, an index into `palette.swatches`.
pub fn map_pixels(
    img: &RgbImage,
    hist: &Histogram,
    palette: &Palette,
    selected: &[usize],
    metric: DistanceMetric,
) -> Vec<u16> {
    let sel_lab: Vec<(usize, Lab)> = selected.iter().map(|&i| (i, palette.swatches[i].lab())).collect();
    let sample_map: Vec<u16> = hist
        .samples
        .par_iter()
        .map(|&(lab, _)| {
            sel_lab
                .iter()
                .min_by(|a, b| metric.distance(a.1, lab).total_cmp(&metric.distance(b.1, lab)))
                .map(|x| x.0 as u16)
                .unwrap_or(0)
        })
        .collect();
    img.pixels().map(|p| sample_map[hist.sample_of(p.0)]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_tone() -> RgbImage {
        RgbImage::from_fn(40, 20, |x, _| {
            if x < 20 {
                image::Rgb([250, 10, 10])
            } else {
                image::Rgb([10, 10, 240])
            }
        })
    }

    #[test]
    fn picks_matching_colors() {
        let img = two_tone();
        let hist = Histogram::build(&img);
        let pal = Palette::basic_acrylic_12();
        let sel = select_palette_subset(&hist, &pal, 2, DistanceMetric::Ciede2000);
        let names: Vec<&str> = sel.iter().map(|&i| pal.swatches[i].name.as_str()).collect();
        assert!(names.contains(&"Cadmium Red"), "{names:?}");
        assert!(names.iter().any(|n| n.contains("Blue")), "{names:?}");
    }

    #[test]
    fn detail_weights_favour_sharp_areas() {
        // left half: fine checkerboard (detail); right half: flat
        let img = RgbImage::from_fn(200, 100, |x, y| {
            if x < 100 && (x / 2 + y / 2) % 2 == 0 {
                image::Rgb([200, 30, 30])
            } else {
                image::Rgb([60, 120, 60])
            }
        });
        let w = detail_weights(&img);
        let mean = |x0: u32, x1: u32| {
            let v: Vec<f32> = (10..90)
                .flat_map(|y| (x0..x1).map(move |x| (y * 200 + x) as usize))
                .map(|i| w[i])
                .collect();
            v.iter().sum::<f32>() / v.len() as f32
        };
        assert!(
            mean(10, 80) > 3.0 * mean(130, 190),
            "{} vs {}",
            mean(10, 80),
            mean(130, 190)
        );
    }

    #[test]
    fn auto_palette_finds_both_colors() {
        let hist = Histogram::build(&two_tone());
        let p = auto_palette(&hist, 2);
        assert_eq!(p.len(), 2);
        let has = |c: Rgb8| p.swatches.iter().any(|s| s.lab().dist2(c.to_lab()) < 25.0);
        assert!(has(Rgb8::new(250, 10, 10)) && has(Rgb8::new(10, 10, 240)), "{p:?}");
    }
}
