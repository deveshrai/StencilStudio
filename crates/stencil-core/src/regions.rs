//! Connected-region segmentation of a color-index map, plus merging of regions
//! too small to cut or paint at the target physical size.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

/// A labeled segmentation: every pixel belongs to exactly one region, and every
/// region has exactly one palette color.
#[derive(Debug, Clone)]
pub struct Segmentation {
    pub width: u32,
    pub height: u32,
    /// Region id per pixel (row-major).
    pub labels: Vec<u32>,
    /// Palette index (into the full palette) per region id.
    pub region_color: Vec<u16>,
    /// Pixel area per region id.
    pub region_area: Vec<u32>,
}

impl Segmentation {
    pub fn region_count(&self) -> usize {
        self.region_color.len()
    }

    /// Palette index per pixel.
    pub fn color_map(&self) -> Vec<u16> {
        self.labels.iter().map(|&l| self.region_color[l as usize]).collect()
    }

    /// Label 4-connected components of equal color.
    pub fn from_color_map(width: u32, height: u32, colors: &[u16]) -> Self {
        let (w, h) = (width as usize, height as usize);
        assert_eq!(colors.len(), w * h);
        let mut labels = vec![u32::MAX; w * h];
        let mut region_color = Vec::new();
        let mut region_area = Vec::new();
        let mut stack = Vec::new();
        for start in 0..w * h {
            if labels[start] != u32::MAX {
                continue;
            }
            let id = region_color.len() as u32;
            let c = colors[start];
            let mut area = 0u32;
            labels[start] = id;
            stack.push(start);
            while let Some(i) = stack.pop() {
                area += 1;
                let (x, y) = (i % w, i / w);
                let mut visit = |j: usize| {
                    if labels[j] == u32::MAX && colors[j] == c {
                        labels[j] = id;
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
            region_color.push(c);
            region_area.push(area);
        }
        Segmentation {
            width,
            height,
            labels,
            region_color,
            region_area,
        }
    }

    /// Merge every region smaller than `min_area` pixels into the neighbor it shares
    /// the longest border with, smallest regions first. Returns a fresh segmentation
    /// in which adjacent same-colored regions are coalesced.
    pub fn merge_small_regions(&self, min_area: u32) -> Segmentation {
        let n = self.region_count();
        if min_area <= 1 || n <= 1 {
            return self.clone();
        }
        let (w, h) = (self.width as usize, self.height as usize);

        // Region adjacency with shared border length.
        let mut adj: Vec<HashMap<u32, u32>> = vec![HashMap::new(); n];
        let mut link = |a: u32, b: u32| {
            if a != b {
                *adj[a as usize].entry(b).or_insert(0) += 1;
                *adj[b as usize].entry(a).or_insert(0) += 1;
            }
        };
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if x + 1 < w {
                    link(self.labels[i], self.labels[i + 1]);
                }
                if y + 1 < h {
                    link(self.labels[i], self.labels[i + w]);
                }
            }
        }

        let mut parent: Vec<u32> = (0..n as u32).collect();
        let mut area = self.region_area.clone();
        let color = &self.region_color;
        let mut heap: BinaryHeap<Reverse<(u32, u32)>> = (0..n as u32)
            .filter(|&r| area[r as usize] < min_area)
            .map(|r| Reverse((area[r as usize], r)))
            .collect();

        while let Some(Reverse((a, r))) = heap.pop() {
            let ri = r as usize;
            if parent[ri] != r || area[ri] != a {
                continue; // merged away or stale entry
            }
            if a >= min_area {
                continue;
            }
            // Pick the neighbor with the longest shared border (ties: larger neighbor).
            let Some((&target, _)) = adj[ri]
                .iter()
                .max_by_key(|&(&nb, &len)| (len, area[nb as usize], Reverse(nb)))
            else {
                continue; // isolated (whole image is one region)
            };
            let ti = target as usize;
            // Fold r's adjacency into target's.
            let r_adj = std::mem::take(&mut adj[ri]);
            for (nb, len) in r_adj {
                let nbi = nb as usize;
                adj[nbi].remove(&r);
                if nb == target {
                    continue;
                }
                *adj[nbi].entry(target).or_insert(0) += len;
                *adj[ti].entry(nb).or_insert(0) += len;
            }
            adj[ti].remove(&r);
            parent[ri] = target;
            area[ti] += a;
            if area[ti] < min_area {
                heap.push(Reverse((area[ti], target)));
            }
        }

        fn find(parent: &mut [u32], mut x: u32) -> u32 {
            while parent[x as usize] != x {
                let p = parent[x as usize];
                parent[x as usize] = parent[p as usize];
                x = p;
            }
            x
        }
        let root_color: Vec<u16> = (0..n as u32).map(|r| color[find(&mut parent, r) as usize]).collect();
        let colors: Vec<u16> = self.labels.iter().map(|&l| root_color[l as usize]).collect();
        Segmentation::from_color_map(self.width, self.height, &colors)
    }

    /// True if pixel `i` lies on a boundary with a different region (right or below).
    pub fn is_boundary(&self, x: u32, y: u32) -> bool {
        let (w, h) = (self.width, self.height);
        let i = (y * w + x) as usize;
        let l = self.labels[i];
        (x + 1 < w && self.labels[i + 1] != l) || (y + 1 < h && self.labels[i + w as usize] != l)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_components() {
        // 0 0 1
        // 1 1 1
        // 0 1 0
        let s = Segmentation::from_color_map(3, 3, &[0, 0, 1, 1, 1, 1, 0, 1, 0]);
        assert_eq!(s.region_count(), 4);
        assert_eq!(s.region_area.iter().sum::<u32>(), 9);
    }

    #[test]
    fn merges_speck_into_surrounding() {
        let (w, h) = (10u32, 10u32);
        let mut c = vec![0u16; 100];
        c[55] = 1; // single-pixel speck
        for y in 0..10 {
            c[y * 10 + 9] = 2; // right column stripe (area 10)
        }
        let s = Segmentation::from_color_map(w, h, &c);
        assert_eq!(s.region_count(), 3);
        let m = s.merge_small_regions(5);
        assert_eq!(m.region_count(), 2);
        assert_eq!(m.color_map()[55], 0);
        assert_eq!(m.color_map()[9], 2);
    }

    #[test]
    fn everything_tiny_collapses_to_one() {
        // checkerboard: every region is 1px
        let c: Vec<u16> = (0..64).map(|i| (((i % 8) + (i / 8)) % 2) as u16).collect();
        let m = Segmentation::from_color_map(8, 8, &c).merge_small_regions(1000);
        assert_eq!(m.region_count(), 1);
    }
}
