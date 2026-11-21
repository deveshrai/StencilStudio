//! Floating-piece handling for stencil sheets: connect islands to the surrounding
//! material with two thin straight bridges, or drop them if they are too small.
//!
//! Works on the solid-material pixel mask *before* tracing, so bridges become part
//! of the outline and every export format (SVG, DXF, STL) gets them.

pub(crate) struct CompInfo {
    pub area: u64,
    pub sum_x: u64,
    pub sum_y: u64,
}

/// 4-connected components of `true` pixels.
pub(crate) struct Components {
    /// Component per pixel (`u32::MAX` for empty pixels).
    pub id: Vec<u32>,
    pub info: Vec<CompInfo>,
}

impl Components {
    pub fn label(mask: &[bool], w: u32, h: u32) -> Self {
        let mut id = vec![u32::MAX; mask.len()];
        let mut info = Vec::new();
        let mut stack = Vec::new();
        for start in 0..mask.len() {
            if !mask[start] || id[start] != u32::MAX {
                continue;
            }
            let c = info.len() as u32;
            let mut ci = CompInfo {
                area: 0,
                sum_x: 0,
                sum_y: 0,
            };
            id[start] = c;
            stack.push(start);
            while let Some(i) = stack.pop() {
                let (x, y) = ((i as u32) % w, (i as u32) / w);
                ci.area += 1;
                ci.sum_x += x as u64;
                ci.sum_y += y as u64;
                let mut visit = |j: usize| {
                    if mask[j] && id[j] == u32::MAX {
                        id[j] = c;
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
                    visit(i - w as usize);
                }
                if y + 1 < h {
                    visit(i + w as usize);
                }
            }
            info.push(ci);
        }
        Components { id, info }
    }

    pub fn count(&self) -> u32 {
        self.info.len() as u32
    }

    pub fn largest(&self) -> u32 {
        (0..self.count())
            .max_by_key(|&c| self.info[c as usize].area)
            .unwrap_or(0)
    }
}

/// How to treat floating pieces (all sizes in pixels).
#[derive(Clone, Copy)]
pub(crate) struct IslandRules {
    /// The sheet has a solid frame (pixel (0,0) belongs to the main sheet).
    pub has_frame: bool,
    pub bridge_px: u32,
    /// Pieces smaller than this are dropped.
    pub min_px: u64,
    /// Pieces at least this big are always bridged, however long the bridges.
    pub always_bridge_px: u64,
    /// A removed-bridge point this close (px) to an automatic bridge suppresses it.
    pub removed_tol_px: f32,
}

/// Bridge or drop every island in `mask` (`w`×`h`). The main sheet is the component
/// containing pixel (0,0) when there is a frame, otherwise the largest one.
/// Bridge rectangles (pixel coordinates, `[x0, y0, x1, y1)` half-open) are appended to
/// `bridges`. Returns (bridged, dropped).
pub(crate) fn resolve_islands(
    mask: &mut [bool],
    w: u32,
    h: u32,
    rules: &IslandRules,
    removed: &[[f32; 2]],
    bridges: &mut Vec<[u32; 4]>,
) -> (usize, usize) {
    let IslandRules {
        has_frame,
        bridge_px,
        min_px,
        always_bridge_px,
        ..
    } = *rules;
    let (mut bridged, mut dropped) = (0, 0);
    // Several passes: a bridge may land on another island, which is handled next pass.
    for _ in 0..12 {
        let comps = Components::label(mask, w, h);
        if comps.count() == 0 {
            break;
        }
        let main = if has_frame && comps.id[0] != u32::MAX {
            comps.id[0]
        } else {
            comps.largest()
        };
        let islands: Vec<u32> = (0..comps.count()).filter(|&c| c != main).collect();
        if islands.is_empty() {
            break;
        }
        let mut progress = false;
        for c in islands {
            let area = comps.info[c as usize].area;
            let plan = plan_bridges(&comps, c, w);
            let total_len: u32 = plan.iter().map(|(p, _)| p.gap()).sum();
            // Small pieces, and mid-size pieces whose bridges would be much longer than
            // the piece itself, are better dropped than held by long spindly bridges.
            let too_long = area < always_bridge_px && total_len as f64 > MAX_BRIDGE_RATIO * (area as f64).sqrt();
            if area < min_px.max(1) || too_long || plan.is_empty() {
                for (m, &id) in mask.iter_mut().zip(&comps.id) {
                    if id == c {
                        *m = false;
                    }
                }
                dropped += 1;
                progress = true;
            } else {
                // Bridges the user removed (clicked away) are skipped; if all of an
                // island's bridges are removed it stays floating, as the user asked.
                let mut painted = 0;
                for (p, horizontal) in &plan {
                    let r = bridge_rect(w, h, p, bridge_px.max(2), *horizontal);
                    if removed.iter().any(|q| rect_near(r, *q, rules.removed_tol_px)) {
                        continue;
                    }
                    fill_rect(mask, w, r);
                    bridges.push(r);
                    painted += 1;
                }
                if painted > 0 {
                    bridged += 1;
                    progress = true;
                }
            }
        }
        if !progress {
            break;
        }
    }
    (bridged, dropped)
}

/// Bridges may be at most this many times the island's size (√area) in total length,
/// unless the island is large.
const MAX_BRIDGE_RATIO: f64 = 4.0;

/// A straight probe from an island to the nearest other solid material.
struct Probe {
    /// Row (horizontal probe) or column (vertical probe).
    line: u32,
    /// Last island pixel before the gap, and the first foreign solid pixel after it.
    from: u32,
    to: u32,
}

impl Probe {
    fn gap(&self) -> u32 {
        self.from.abs_diff(self.to)
    }
}

/// Walk from `start` along a row/column until reaching solid material of another component.
fn probe(comps: &Components, island: u32, w: u32, line: u32, start: u32, step: i64, horizontal: bool) -> Option<Probe> {
    let len = if horizontal { w } else { comps.id.len() as u32 / w };
    let at = |t: u32| -> usize {
        if horizontal {
            (line * w + t) as usize
        } else {
            (t * w + line) as usize
        }
    };
    let mut edge = start;
    let mut t = start as i64;
    loop {
        t += step;
        if t < 0 || t >= len as i64 {
            return None;
        }
        let c = comps.id[at(t as u32)];
        if c == island {
            edge = t as u32; // concave island: the gap restarts here
        } else if c != u32::MAX {
            return Some(Probe {
                line,
                from: edge,
                to: t as u32,
            });
        }
    }
}

/// Two bridges on opposite sides of the island (left+right or up+down, whichever is
/// shorter in total); a single bridge if only one direction reaches material.
fn plan_bridges(comps: &Components, island: u32, w: u32) -> Vec<(Probe, bool)> {
    let info = &comps.info[island as usize];
    let (cx, cy) = ((info.sum_x / info.area) as i64, (info.sum_y / info.area) as i64);
    // Anchor both probe lines on the island pixel closest to its centroid.
    let (mut ax, mut ay, mut best) = (0u32, 0u32, i64::MAX);
    for (i, &c) in comps.id.iter().enumerate() {
        if c == island {
            let (x, y) = ((i as u32) % w, (i as u32) / w);
            let d = (x as i64 - cx).pow(2) + (y as i64 - cy).pow(2);
            if d < best {
                best = d;
                (ax, ay) = (x, y);
            }
        }
    }
    let [l, r] = [
        probe(comps, island, w, ay, ax, -1, true),
        probe(comps, island, w, ay, ax, 1, true),
    ];
    let [u, d] = [
        probe(comps, island, w, ax, ay, -1, false),
        probe(comps, island, w, ax, ay, 1, false),
    ];
    match (l, r, u, d) {
        (Some(l), Some(r), Some(u), Some(d)) => {
            if u.gap() + d.gap() < l.gap() + r.gap() {
                vec![(u, false), (d, false)]
            } else {
                vec![(l, true), (r, true)]
            }
        }
        (Some(l), Some(r), _, _) => vec![(l, true), (r, true)],
        (_, _, Some(u), Some(d)) => vec![(u, false), (d, false)],
        (l, r, u, d) => [
            l.map(|p| (p, true)),
            r.map(|p| (p, true)),
            u.map(|p| (p, false)),
            d.map(|p| (p, false)),
        ]
        .into_iter()
        .flatten()
        .min_by_key(|(p, _)| p.gap())
        .into_iter()
        .collect(),
    }
}

/// Rectangle `[x0, y0, x1, y1)` covered by the bridge for probe `p`.
fn bridge_rect(w: u32, h: u32, p: &Probe, bw: u32, horizontal: bool) -> [u32; 4] {
    let (span, across) = if horizontal { (w, h) } else { (h, w) };
    // Overlap 2 px into both ends so the joint stays solid on slanted edges.
    let lo = p.from.min(p.to).saturating_sub(2);
    let hi = (p.from.max(p.to) + 2).min(span - 1);
    let a0 = p.line.saturating_sub(bw / 2);
    let a1 = (a0 + bw).min(across);
    if horizontal {
        [lo, a0, hi + 1, a1]
    } else {
        [a0, lo, a1, hi + 1]
    }
}

fn fill_rect(mask: &mut [bool], w: u32, r: [u32; 4]) {
    for y in r[1]..r[3] {
        for x in r[0]..r[2] {
            mask[(y * w + x) as usize] = true;
        }
    }
}

fn rect_near(r: [u32; 4], q: [f32; 2], tol: f32) -> bool {
    q[0] >= r[0] as f32 - tol && q[0] <= r[2] as f32 + tol && q[1] >= r[1] as f32 - tol && q[1] <= r[3] as f32 + tol
}

/// A user-placed bridge across the cut at `(x, y)`: spans to the nearest material
/// on both sides, horizontally or vertically (whichever is shorter). Returns `None`
/// if the point is on material or no direction reaches material on both sides.
pub(crate) fn manual_bridge(mask: &mut [bool], w: u32, h: u32, x: f32, y: f32, bw: u32) -> Option<[u32; 4]> {
    let (x, y) = (x.floor() as i64, y.floor() as i64);
    if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 || mask[(y * w as i64 + x) as usize] {
        return None;
    }
    // distance along a direction to the first solid pixel
    let reach = |dx: i64, dy: i64| -> Option<i64> {
        let (mut cx, mut cy, mut n) = (x, y, 0);
        loop {
            cx += dx;
            cy += dy;
            n += 1;
            if cx < 0 || cy < 0 || cx >= w as i64 || cy >= h as i64 {
                return None;
            }
            if mask[(cy * w as i64 + cx) as usize] {
                return Some(n);
            }
        }
    };
    let hor = reach(-1, 0).zip(reach(1, 0));
    let ver = reach(0, -1).zip(reach(0, 1));
    let horizontal = match (hor, ver) {
        (Some((l, r)), Some((u, d))) => l + r <= u + d,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => return None,
    };
    let (a, b) = if horizontal { hor.unwrap() } else { ver.unwrap() };
    let (line, from, to) = if horizontal {
        (y as u32, (x - a) as u32, (x + b) as u32)
    } else {
        (x as u32, (y - a) as u32, (y + b) as u32)
    };
    let r = bridge_rect(w, h, &Probe { line, from, to }, bw.max(2), horizontal);
    fill_rect(mask, w, r);
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(bridge_px: u32, min_px: u64, always_bridge_px: u64) -> IslandRules {
        IslandRules {
            has_frame: true,
            bridge_px,
            min_px,
            always_bridge_px,
            removed_tol_px: 3.0,
        }
    }

    /// 40x40 solid sheet with a ring-shaped cut (r 8..14) around a solid centre.
    fn ring_mask() -> Vec<bool> {
        (0..40 * 40)
            .map(|i| {
                let (x, y) = ((i % 40) as f32 - 20.0, (i / 40) as f32 - 20.0);
                let r = (x * x + y * y).sqrt();
                !(8.0..14.0).contains(&r)
            })
            .collect()
    }

    #[test]
    fn bridges_ring_centre() {
        let mut m = ring_mask();
        assert_eq!(Components::label(&m, 40, 40).count(), 2);
        assert_eq!(
            resolve_islands(&mut m, 40, 40, &rules(3, 4, 0), &[], &mut Vec::new()),
            (1, 0)
        );
        assert_eq!(Components::label(&m, 40, 40).count(), 1);
    }

    #[test]
    fn drops_speck_with_long_bridges() {
        // 3x3 speck in the middle of a 60x60 hole: bridges would be ~30 px each side
        let mut m: Vec<bool> = (0..100 * 100)
            .map(|i| {
                let (x, y) = (i % 100, i / 100);
                let in_hole = (20..80).contains(&x) && (20..80).contains(&y);
                let speck = (49..52).contains(&x) && (49..52).contains(&y);
                !in_hole || speck
            })
            .collect();
        assert_eq!(
            resolve_islands(&mut m, 100, 100, &rules(2, 4, 10_000), &[], &mut Vec::new()),
            (0, 1)
        );
    }

    #[test]
    fn removed_bridge_is_skipped_and_manual_bridge_spans_cut() {
        let mut m = ring_mask();
        let mut b = Vec::new();
        assert_eq!(resolve_islands(&mut m, 40, 40, &rules(3, 4, 0), &[], &mut b), (1, 0));
        assert_eq!(b.len(), 2);
        // remove one of the two bridges: the other still holds the centre
        let r = b[0];
        let centre = [(r[0] + r[2]) as f32 / 2.0, (r[1] + r[3]) as f32 / 2.0];
        let mut m = ring_mask();
        let mut b2 = Vec::new();
        assert_eq!(
            resolve_islands(&mut m, 40, 40, &rules(3, 4, 0), &[centre], &mut b2),
            (1, 0)
        );
        assert_eq!(b2.len(), 1);
        // remove both: the centre stays floating
        let r2 = b[1];
        let c2 = [(r2[0] + r2[2]) as f32 / 2.0, (r2[1] + r2[3]) as f32 / 2.0];
        let mut m = ring_mask();
        assert_eq!(
            resolve_islands(&mut m, 40, 40, &rules(3, 4, 0), &[centre, c2], &mut Vec::new()),
            (0, 0)
        );
        assert_eq!(Components::label(&m, 40, 40).count(), 2);
        // a manual bridge clicked in the ring cut connects it again
        let br = manual_bridge(&mut m, 40, 40, 20.5, 9.5, 3).expect("bridge");
        assert!(br[3] - br[1] > 3 || br[2] - br[0] > 3);
        assert_eq!(Components::label(&m, 40, 40).count(), 1);
        // clicking on material does nothing
        assert!(manual_bridge(&mut m, 40, 40, 2.0, 2.0, 3).is_none());
    }

    #[test]
    fn drops_small_island() {
        let mut m = ring_mask();
        assert_eq!(
            resolve_islands(&mut m, 40, 40, &rules(3, 10_000, 0), &[], &mut Vec::new()),
            (0, 1)
        );
        let c = Components::label(&m, 40, 40);
        assert_eq!(c.count(), 1);
        assert!(!m[20 * 40 + 20]); // centre is now cut away
    }
}
