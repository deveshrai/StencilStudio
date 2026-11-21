//! Stencil sheet geometry: one sheet per color layer, with a solid frame,
//! registration holes, and cut-outs where that color is painted.

use crate::color::Rgb8;
use crate::palette::Palette;
use crate::regions::Segmentation;
use crate::vector::{Pt, SmoothBoundaries, simplify_closed, trace_mask};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StencilOptions {
    /// Sheet thickness for 3D printing (mm).
    pub thickness_mm: f32,
    /// Solid border around the image (mm).
    pub frame_mm: f32,
    /// Punch identical round registration holes in the frame corners of every layer.
    pub registration_marks: bool,
    /// Side length of a registration hole (mm).
    pub mark_size_mm: f32,
    /// Max deviation allowed when simplifying curves (mm).
    pub tolerance_mm: f32,
}

impl Default for StencilOptions {
    fn default() -> Self {
        Self {
            thickness_mm: 1.0,
            frame_mm: 12.0,
            registration_marks: true,
            mark_size_mm: 4.0,
            tolerance_mm: 0.02,
        }
    }
}

/// A solid piece of a stencil sheet: an outline plus the holes cut into it.
/// Coordinates are millimetres, origin top-left, y pointing down.
#[derive(Debug, Clone)]
pub struct SolidPolygon {
    pub outer: Vec<Pt>,
    pub holes: Vec<Vec<Pt>>,
}

#[derive(Debug, Clone)]
pub struct StencilLayer {
    /// 1-based painting order (lightest first).
    pub number: usize,
    pub palette_index: u16,
    pub name: String,
    pub color: Rgb8,
    /// Full sheet size including the frame (mm).
    pub sheet_mm: (f32, f32),
    /// Solid material. `pieces[0]` is the main sheet; any further pieces are
    /// floating islands that would fall out unless bridged.
    pub pieces: Vec<SolidPolygon>,
}

impl StencilLayer {
    pub fn island_count(&self) -> usize {
        self.pieces.len().saturating_sub(1)
    }

    /// Every closed cut path (outer outlines and holes).
    pub fn cut_paths(&self) -> impl Iterator<Item = &Vec<Pt>> {
        self.pieces
            .iter()
            .flat_map(|p| std::iter::once(&p.outer).chain(p.holes.iter()))
    }

    /// Where to place the layer label (inside the bottom frame), and its font size, in mm.
    pub fn label_anchor(&self, frame_mm: f32) -> Option<(Pt, f32)> {
        if frame_mm < 4.0 {
            return None;
        }
        let size = (frame_mm * 0.35).min(5.0);
        Some(([frame_mm, self.sheet_mm.1 - frame_mm / 2.0 + size * 0.35], size))
    }

    pub fn label(&self) -> String {
        format!("Layer {} - {}", self.number, self.name)
    }
}

/// Build stencil layers for every palette color used in `seg`.
pub fn build_layers(
    seg: &Segmentation,
    sb: &SmoothBoundaries,
    palette: &Palette,
    px_per_mm: f32,
    opts: &StencilOptions,
) -> Vec<StencilLayer> {
    let mut used: Vec<u16> = seg.region_color.clone();
    used.sort_unstable();
    used.dedup();
    // Paint light colors first.
    used.sort_by(|a, b| {
        let la = palette.swatches[*a as usize].lab().l;
        let lb = palette.swatches[*b as usize].lab().l;
        lb.total_cmp(&la)
    });
    used.iter()
        .enumerate()
        .map(|(i, &c)| {
            let s = &palette.swatches[c as usize];
            let mut layer = build_layer(seg, sb, c, px_per_mm, opts);
            layer.number = i + 1;
            layer.name = s.name.clone();
            layer.color = s.color;
            layer
        })
        .collect()
}

fn build_layer(
    seg: &Segmentation,
    sb: &SmoothBoundaries,
    color: u16,
    px_per_mm: f32,
    opts: &StencilOptions,
) -> StencilLayer {
    let (w, h) = (seg.width, seg.height);
    let f = (opts.frame_mm * px_per_mm).round().max(0.0) as u32;
    let (pw, ph) = (w + 2 * f, h + 2 * f);

    let solid = |x: u32, y: u32| -> bool {
        if x < f || y < f || x >= f + w || y >= f + h {
            true
        } else {
            seg.region_color[seg.labels[((y - f) * w + (x - f)) as usize] as usize] != color
        }
    };

    let loops = trace_mask(pw, ph, solid);

    // Connected components of solid pixels (4-connected) to group holes with outlines.
    let mut comp = vec![u32::MAX; (pw * ph) as usize];
    let mut stack = Vec::new();
    let mut ncomp = 0u32;
    let comp_of = |comp: &mut Vec<u32>, stack: &mut Vec<u32>, ncomp: &mut u32, sx: u32, sy: u32| -> u32 {
        let si = (sy * pw + sx) as usize;
        if comp[si] != u32::MAX {
            return comp[si];
        }
        let id = *ncomp;
        *ncomp += 1;
        comp[si] = id;
        stack.push(si as u32);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % pw, i / pw);
            let mut visit = |nx: u32, ny: u32| {
                let j = (ny * pw + nx) as usize;
                if comp[j] == u32::MAX && solid(nx, ny) {
                    comp[j] = id;
                    stack.push(j as u32);
                }
            };
            if x > 0 {
                visit(x - 1, y);
            }
            if x + 1 < pw {
                visit(x + 1, y);
            }
            if y > 0 {
                visit(x, y - 1);
            }
            if y + 1 < ph {
                visit(x, y + 1);
            }
        }
        id
    };

    let to_mm = |cx: u32, cy: u32| -> Pt {
        let p = if cx >= f && cy >= f && cx <= f + w && cy <= f + h {
            let q = sb.corner(cx - f, cy - f);
            [q[0] + f as f32, q[1] + f as f32]
        } else {
            [cx as f32, cy as f32]
        };
        [p[0] / px_per_mm, p[1] / px_per_mm]
    };

    // component id -> (outer outline, holes)
    type Group = (Option<Vec<Pt>>, Vec<Vec<Pt>>);
    let mut groups: HashMap<u32, Group> = HashMap::new();
    let mut order = Vec::new();
    for l in &loops {
        let id = comp_of(&mut comp, &mut stack, &mut ncomp, l.inside_pixel.0, l.inside_pixel.1);
        let pts: Vec<Pt> = l.corners.iter().map(|&(x, y)| to_mm(x, y)).collect();
        let pts = simplify_closed(&pts, opts.tolerance_mm);
        let g = groups.entry(id).or_insert_with(|| {
            order.push(id);
            (None, Vec::new())
        });
        if l.outer {
            g.0 = Some(pts);
        } else {
            g.1.push(pts);
        }
    }
    let mut pieces: Vec<SolidPolygon> = order
        .into_iter()
        .filter_map(|id| {
            let (outer, holes) = groups.remove(&id)?;
            Some(SolidPolygon { outer: outer?, holes })
        })
        .collect();
    // Main sheet first (largest outline).
    pieces.sort_by(|a, b| {
        crate::vector::signed_area(&b.outer)
            .abs()
            .total_cmp(&crate::vector::signed_area(&a.outer).abs())
    });

    // Registration holes: identical circles centred in each frame corner (fit alignment pins).
    let sheet_mm = (pw as f32 / px_per_mm, ph as f32 / px_per_mm);
    let frame_mm = f as f32 / px_per_mm;
    let r = opts.mark_size_mm / 2.0;
    if opts.registration_marks && r > 0.0 && frame_mm >= opts.mark_size_mm + 2.0 && !pieces.is_empty() {
        let c = frame_mm / 2.0;
        for (cx, cy) in [
            (c, c),
            (sheet_mm.0 - c, c),
            (c, sheet_mm.1 - c),
            (sheet_mm.0 - c, sheet_mm.1 - c),
        ] {
            let circle: Vec<Pt> = (0..32)
                .map(|i| {
                    let a = -(i as f32) * std::f32::consts::TAU / 32.0;
                    [cx + r * a.cos(), cy + r * a.sin()]
                })
                .collect();
            pieces[0].holes.push(circle);
        }
    }

    StencilLayer {
        number: 0,
        palette_index: color,
        name: String::new(),
        color: Rgb8::BLACK,
        sheet_mm,
        pieces,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::Swatch;

    #[test]
    fn ring_produces_island() {
        // a ring of color 1 on background 0 -> the ring's centre is a floating island
        let (w, h) = (60u32, 60u32);
        let colors: Vec<u16> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 - 30.0, (i / w) as f32 - 30.0);
                let r = (x * x + y * y).sqrt();
                u16::from((10.0..20.0).contains(&r))
            })
            .collect();
        let seg = Segmentation::from_color_map(w, h, &colors);
        let sb = SmoothBoundaries::build(&seg, 1.0);
        let pal = Palette {
            name: "t".into(),
            swatches: vec![Swatch::new("White", Rgb8::WHITE), Swatch::new("Black", Rgb8::BLACK)],
        };
        let layers = build_layers(&seg, &sb, &pal, 2.0, &StencilOptions::default());
        assert_eq!(layers.len(), 2);
        let black = layers.iter().find(|l| l.name == "Black").unwrap();
        assert_eq!(black.number, 2); // white is painted first
        assert_eq!(black.island_count(), 1);
        // main sheet: frame outline, 1 ring hole, 4 registration holes
        assert_eq!(black.pieces[0].holes.len(), 5);
        assert!((black.sheet_mm.0 - (30.0 + 24.0)).abs() < 0.6);
    }
}
