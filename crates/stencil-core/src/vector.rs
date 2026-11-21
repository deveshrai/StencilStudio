//! Smooth vector boundaries for a segmentation.
//!
//! Every boundary between two regions (or a region and the image edge) is split
//! into *chains* that run between *junctions* (pixel corners where 3+ regions meet,
//! diagonal saddles, and image corners). Each chain is smoothed exactly once with
//! its end points pinned, which yields one smoothed position per boundary corner.
//! Any outline traced later — a single region, a whole stencil layer, or every
//! paint-by-numbers cell — reads those shared positions, so neighbouring shapes
//! always meet exactly, with no gaps or overlaps.

use crate::regions::Segmentation;

pub type Pt = [f32; 2];

const OUTSIDE: u32 = u32::MAX;

/// Smoothed boundary geometry in image pixel coordinates (corner grid).
pub struct SmoothBoundaries {
    pub width: u32,
    pub height: u32,
    /// Position of every pixel corner, `(w+1) * (h+1)`, row-major. Corners not on
    /// a boundary keep their integer position.
    pos: Vec<Pt>,
    /// Every smoothed chain as an open (or closed, first == last) polyline.
    pub chains: Vec<Vec<Pt>>,
}

impl SmoothBoundaries {
    #[inline]
    pub fn corner(&self, cx: u32, cy: u32) -> Pt {
        self.pos[(cy * (self.width + 1) + cx) as usize]
    }

    /// Build and smooth all boundaries. `sigma_px` is the Gaussian smoothing radius
    /// in pixels (0 = keep the pixel staircase).
    pub fn build(seg: &Segmentation, sigma_px: f32) -> Self {
        let (w, h) = (seg.width as i64, seg.height as i64);
        let label = |x: i64, y: i64| -> u32 {
            if x < 0 || y < 0 || x >= w || y >= h {
                OUTSIDE
            } else {
                seg.labels[(y * w + x) as usize]
            }
        };
        let cw = (w + 1) as usize;
        let cidx = |cx: i64, cy: i64| (cy as usize) * cw + cx as usize;

        // Is the edge leaving corner (cx,cy) in direction d a boundary?
        // d: 0 = right(+x), 1 = down(+y), 2 = left(-x), 3 = up(-y)
        let edge_is_boundary = |cx: i64, cy: i64, d: u8| -> bool {
            match d {
                0 => cx < w && label(cx, cy - 1) != label(cx, cy),
                1 => cy < h && label(cx - 1, cy) != label(cx, cy),
                2 => cx > 0 && label(cx - 1, cy - 1) != label(cx - 1, cy),
                _ => cy > 0 && label(cx - 1, cy - 1) != label(cx, cy - 1),
            }
        };
        const DX: [i64; 4] = [1, 0, -1, 0];
        const DY: [i64; 4] = [0, 1, 0, -1];

        let degree = |cx: i64, cy: i64| (0..4).filter(|&d| edge_is_boundary(cx, cy, d)).count();
        let is_corner = |cx: i64, cy: i64| (cx == 0 || cx == w) && (cy == 0 || cy == h);
        let is_junction = |cx: i64, cy: i64| {
            let deg = degree(cx, cy);
            deg != 2 && deg != 0 || is_corner(cx, cy)
        };

        // Visited flags for undirected edges: horizontal edge (cx,cy)-(cx+1,cy) and
        // vertical edge (cx,cy)-(cx,cy+1).
        struct Seen {
            h: Vec<bool>,
            v: Vec<bool>,
            w: usize,
            cw: usize,
        }
        impl Seen {
            fn slot(&mut self, cx: i64, cy: i64, d: u8) -> &mut bool {
                match d {
                    0 => &mut self.h[cy as usize * self.w + cx as usize],
                    2 => &mut self.h[cy as usize * self.w + (cx - 1) as usize],
                    1 => &mut self.v[cy as usize * self.cw + cx as usize],
                    _ => &mut self.v[(cy - 1) as usize * self.cw + cx as usize],
                }
            }
        }
        let mut seen = Seen {
            h: vec![false; (h as usize + 1) * w as usize],
            v: vec![false; cw * h as usize],
            w: w as usize,
            cw,
        };

        let mut raw_chains: Vec<(Vec<(i64, i64)>, bool)> = Vec::new();

        // Walk from (sx,sy) along direction d0 until a junction (or back to start).
        let walk = |sx: i64, sy: i64, d0: u8, seen: &mut Seen| {
            let mut pts = vec![(sx, sy)];
            let (mut cx, mut cy, mut d) = (sx, sy, d0);
            loop {
                *seen.slot(cx, cy, d) = true;
                cx += DX[d as usize];
                cy += DY[d as usize];
                pts.push((cx, cy));
                if (cx, cy) == (sx, sy) || is_junction(cx, cy) {
                    break;
                }
                // degree-2 vertex: continue along the other boundary edge
                let back = (d + 2) % 4;
                let next = (0..4u8).find(|&nd| nd != back && edge_is_boundary(cx, cy, nd) && !*seen.slot(cx, cy, nd));
                match next {
                    Some(nd) => d = nd,
                    None => break,
                }
            }
            let closed = pts.first() == pts.last() && pts.len() > 2;
            (pts, closed)
        };

        // 1) chains that start at junctions
        for cy in 0..=h {
            for cx in 0..=w {
                if degree(cx, cy) == 0 || !is_junction(cx, cy) {
                    continue;
                }
                for d in 0..4u8 {
                    if edge_is_boundary(cx, cy, d) && !*seen.slot(cx, cy, d) {
                        raw_chains.push(walk(cx, cy, d, &mut seen));
                    }
                }
            }
        }
        // 2) closed loops without any junction
        for cy in 0..=h {
            for cx in 0..=w {
                for d in [0u8, 1] {
                    if edge_is_boundary(cx, cy, d) && !*seen.slot(cx, cy, d) {
                        raw_chains.push(walk(cx, cy, d, &mut seen));
                    }
                }
            }
        }

        let mut pos: Vec<Pt> = Vec::with_capacity(cw * (h as usize + 1));
        for cy in 0..=h {
            for cx in 0..=w {
                pos.push([cx as f32, cy as f32]);
            }
        }

        let mut chains = Vec::with_capacity(raw_chains.len());
        for (pts, closed) in raw_chains {
            let fpts: Vec<Pt> = pts.iter().map(|&(x, y)| [x as f32, y as f32]).collect();
            let smoothed = smooth_chain(&fpts, closed, sigma_px);
            // interior vertices get their smoothed position; junction ends stay put
            let (lo, hi) = if closed {
                (0, pts.len() - 1)
            } else {
                (1, pts.len().saturating_sub(1))
            };
            for i in lo..hi {
                let (x, y) = pts[i];
                pos[cidx(x, y)] = smoothed[i];
            }
            chains.push(smoothed);
        }

        SmoothBoundaries {
            width: seg.width,
            height: seg.height,
            pos,
            chains,
        }
    }
}

/// Gaussian smoothing of a polyline. Open chains keep their end points fixed;
/// closed chains (first == last) wrap around. The radius is reduced for short
/// chains so small shapes do not collapse.
fn smooth_chain(pts: &[Pt], closed: bool, sigma: f32) -> Vec<Pt> {
    let n = pts.len();
    if sigma <= 0.05 || n < 3 {
        return pts.to_vec();
    }
    let sigma = sigma.min(n as f32 / 6.0).max(0.0);
    if sigma <= 0.3 {
        return pts.to_vec();
    }
    let r = (sigma * 2.5).ceil() as i64;
    let kernel: Vec<f32> = (-r..=r)
        .map(|k| (-(k * k) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let mut out = pts.to_vec();
    if closed {
        let m = (n - 1) as i64; // unique points
        for i in 0..m {
            let (mut sx, mut sy, mut sw) = (0.0, 0.0, 0.0);
            for (ki, k) in (-r..=r).enumerate() {
                let p = pts[(i + k).rem_euclid(m) as usize];
                sx += p[0] * kernel[ki];
                sy += p[1] * kernel[ki];
                sw += kernel[ki];
            }
            out[i as usize] = [sx / sw, sy / sw];
        }
        out[n - 1] = out[0];
    } else {
        let last = n as i64 - 1;
        for i in 1..last {
            // shrink the window near the ends so pinned end points are respected
            let rr = r.min(i).min(last - i);
            let (mut sx, mut sy, mut sw) = (0.0, 0.0, 0.0);
            for k in -rr..=rr {
                let wgt = kernel[(k + r) as usize];
                let p = pts[(i + k) as usize];
                sx += p[0] * wgt;
                sy += p[1] * wgt;
                sw += wgt;
            }
            out[i as usize] = [sx / sw, sy / sw];
        }
    }
    out
}

/// A closed contour traced around a binary mask on the pixel-corner grid.
#[derive(Debug, Clone)]
pub struct CornerLoop {
    /// Corner coordinates (cx, cy); the first point is not repeated at the end.
    pub corners: Vec<(u32, u32)>,
    /// A pixel inside the mask that touches this loop (used to group holes with
    /// their enclosing outline).
    pub inside_pixel: (u32, u32),
    /// True for outer outlines, false for holes.
    pub outer: bool,
}

/// Trace all boundaries of a binary mask (`inside(x, y)`, outside the grid is
/// "not inside"). Outlines run with the inside on their right in y-down
/// coordinates; diagonal saddles keep inside pixels 4-connected.
pub fn trace_mask(width: u32, height: u32, inside: impl Fn(u32, u32) -> bool) -> Vec<CornerLoop> {
    use std::collections::HashMap;
    let (w, h) = (width as i64, height as i64);
    let ins = |x: i64, y: i64| x >= 0 && y >= 0 && x < w && y < h && inside(x as u32, y as u32);

    // Directed boundary edges: start corner -> list of (end corner, inside pixel)
    type C = (i64, i64);
    let mut out_edges: HashMap<C, Vec<(C, C)>> = HashMap::new();
    for y in 0..h {
        for x in 0..w {
            if !ins(x, y) {
                continue;
            }
            if !ins(x, y - 1) {
                out_edges.entry((x, y)).or_default().push(((x + 1, y), (x, y)));
            }
            if !ins(x + 1, y) {
                out_edges.entry((x + 1, y)).or_default().push(((x + 1, y + 1), (x, y)));
            }
            if !ins(x, y + 1) {
                out_edges.entry((x + 1, y + 1)).or_default().push(((x, y + 1), (x, y)));
            }
            if !ins(x - 1, y) {
                out_edges.entry((x, y + 1)).or_default().push(((x, y), (x, y)));
            }
        }
    }

    let mut loops = Vec::new();
    let mut starts: Vec<C> = out_edges.keys().copied().collect();
    starts.sort_unstable_by_key(|&(x, y)| (y, x));
    for s in starts {
        while let Some(list) = out_edges.get_mut(&s) {
            let Some((first_end, first_px)) = list.pop() else { break };
            let mut corners = vec![(s.0 as u32, s.1 as u32)];
            let mut prev = s;
            let mut cur = first_end;
            let mut area2: i64 = s.0 * cur.1 - cur.0 * s.1;
            while cur != s {
                corners.push((cur.0 as u32, cur.1 as u32));
                let din = (cur.0 - prev.0, cur.1 - prev.1);
                let list = out_edges.get_mut(&cur).expect("broken contour");
                let pick = if list.len() == 1 {
                    0
                } else {
                    // saddle: take the right turn (keeps inside 4-connected)
                    let right = (-din.1, din.0);
                    list.iter()
                        .position(|&(e, _)| (e.0 - cur.0, e.1 - cur.1) == right)
                        .unwrap_or(0)
                };
                let (next, _) = list.swap_remove(pick);
                area2 += cur.0 * next.1 - next.0 * cur.1;
                prev = cur;
                cur = next;
            }
            loops.push(CornerLoop {
                corners,
                inside_pixel: (first_px.0 as u32, first_px.1 as u32),
                outer: area2 > 0,
            });
        }
    }
    loops
}

/// Ramer–Douglas–Peucker simplification of a closed polygon (no repeated end point).
pub fn simplify_closed(pts: &[Pt], tolerance: f32) -> Vec<Pt> {
    if pts.len() < 8 || tolerance <= 0.0 {
        return pts.to_vec();
    }
    // split at the point farthest from pts[0] so both halves are open polylines
    let far = (1..pts.len())
        .max_by(|&a, &b| dist2(pts[a], pts[0]).total_cmp(&dist2(pts[b], pts[0])))
        .unwrap();
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[far] = true;
    let ring: Vec<Pt> = pts.iter().copied().chain(std::iter::once(pts[0])).collect();
    let mut keep_ring = vec![false; ring.len()];
    rdp(&ring, 0, far, tolerance * tolerance, &mut keep_ring);
    rdp(&ring, far, ring.len() - 1, tolerance * tolerance, &mut keep_ring);
    for (i, k) in keep_ring.iter().enumerate().take(pts.len()) {
        keep[i] |= *k;
    }
    let out: Vec<Pt> = pts.iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| *p).collect();
    if out.len() < 3 { pts.to_vec() } else { out }
}

fn dist2(a: Pt, b: Pt) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

fn seg_dist2(p: Pt, a: Pt, b: Pt) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    if len2 == 0.0 {
        return dist2(p, a);
    }
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0);
    dist2(p, [a[0] + t * dx, a[1] + t * dy])
}

fn rdp(pts: &[Pt], first: usize, last: usize, tol2: f32, keep: &mut [bool]) {
    let mut stack = vec![(first, last)];
    while let Some((a, b)) = stack.pop() {
        keep[a] = true;
        keep[b] = true;
        if b <= a + 1 {
            continue;
        }
        let (mut best, mut best_d) = (0, -1.0);
        for i in a + 1..b {
            let d = seg_dist2(pts[i], pts[a], pts[b]);
            if d > best_d {
                best = i;
                best_d = d;
            }
        }
        if best_d > tol2 {
            stack.push((a, best));
            stack.push((best, b));
        }
    }
}

/// Simplify an open polyline (end points kept).
pub fn simplify_open(pts: &[Pt], tolerance: f32) -> Vec<Pt> {
    if pts.len() < 3 || tolerance <= 0.0 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    rdp(pts, 0, pts.len() - 1, tolerance * tolerance, &mut keep);
    pts.iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| *p).collect()
}

/// Signed area (y-down: positive = clockwise on screen).
pub fn signed_area(pts: &[Pt]) -> f32 {
    let n = pts.len();
    (0..n)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f32>()
        / 2.0
}

/// Smoothed outlines of all pixels whose palette color is `color`.
pub fn color_contours(seg: &Segmentation, sb: &SmoothBoundaries, color: u16, tolerance: f32) -> Vec<Vec<Pt>> {
    let w = seg.width;
    trace_mask(seg.width, seg.height, |x, y| {
        seg.region_color[seg.labels[(y * w + x) as usize] as usize] == color
    })
    .into_iter()
    .map(|l| {
        let pts: Vec<Pt> = l.corners.iter().map(|&(cx, cy)| sb.corner(cx, cy)).collect();
        simplify_closed(&pts, tolerance)
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(w: u32, h: u32, r: f32) -> Segmentation {
        let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
        let colors: Vec<u16> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                u16::from((x - cx).powi(2) + (y - cy).powi(2) < r * r)
            })
            .collect();
        Segmentation::from_color_map(w, h, &colors)
    }

    #[test]
    fn traces_square_with_hole() {
        // 5x5 inside, 1x1 hole in the middle
        let loops = trace_mask(7, 7, |x, y| {
            (1..6).contains(&x) && (1..6).contains(&y) && (x, y) != (3, 3)
        });
        assert_eq!(loops.len(), 2);
        assert_eq!(loops.iter().filter(|l| l.outer).count(), 1);
        let outer = loops.iter().find(|l| l.outer).unwrap();
        assert_eq!(outer.corners.len(), 20);
    }

    #[test]
    fn saddle_is_split_into_two_loops() {
        // diagonal pixels (0,0) and (1,1)
        let loops = trace_mask(2, 2, |x, y| x == y);
        assert_eq!(loops.len(), 2);
        assert!(loops.iter().all(|l| l.outer && l.corners.len() == 4));
    }

    #[test]
    fn smoothing_makes_disk_round_and_keeps_area() {
        let seg = disk(80, 80, 25.0);
        let sb = SmoothBoundaries::build(&seg, 2.0);
        let c = color_contours(&seg, &sb, 1, 0.1);
        assert_eq!(c.len(), 1);
        let area = signed_area(&c[0]).abs();
        let ideal = std::f32::consts::PI * 25.0 * 25.0;
        assert!((area - ideal).abs() / ideal < 0.03, "area {area} vs {ideal}");
        // every point lies close to the true circle
        for p in &c[0] {
            let r = ((p[0] - 40.0).powi(2) + (p[1] - 40.0).powi(2)).sqrt();
            assert!((r - 25.0).abs() < 0.8, "r = {r}");
        }
    }

    #[test]
    fn neighbours_share_identical_boundaries() {
        let seg = disk(60, 60, 18.0);
        let sb = SmoothBoundaries::build(&seg, 2.0);
        let inner = color_contours(&seg, &sb, 1, 0.0);
        let outer = color_contours(&seg, &sb, 0, 0.0);
        // the background's hole must use exactly the disk's points
        let hole = outer.iter().find(|c| c.len() == inner[0].len()).expect("hole");
        let mut a: Vec<(i32, i32)> = inner[0]
            .iter()
            .map(|p| ((p[0] * 1e3) as i32, (p[1] * 1e3) as i32))
            .collect();
        let mut b: Vec<(i32, i32)> = hole
            .iter()
            .map(|p| ((p[0] * 1e3) as i32, (p[1] * 1e3) as i32))
            .collect();
        a.sort();
        b.sort();
        assert_eq!(a, b);
    }
}
