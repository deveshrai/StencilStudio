//! Stencil sheet geometry: one sheet per color layer, with a solid frame,
//! registration holes, and cut-outs where that color is painted.

use crate::color::Rgb8;
use crate::islands::{IslandRules, resolve_islands};
use crate::palette::Palette;
use crate::regions::Segmentation;
use crate::vector::{Pt, SmoothBoundaries, simplify_closed, trace_mask};
use serde::{Deserialize, Serialize};

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
    /// Connect floating pieces to the sheet with two thin bridges.
    pub bridge_islands: bool,
    /// Width of each bridge (mm).
    pub bridge_width_mm: f32,
    /// Floating pieces smaller than this (mm²) are dropped instead of bridged. Mid-size
    /// pieces whose bridges would be far longer than the piece are dropped too.
    pub min_island_mm2: f32,
}

impl Default for StencilOptions {
    fn default() -> Self {
        Self {
            thickness_mm: 1.0,
            frame_mm: 12.0,
            registration_marks: true,
            mark_size_mm: 4.0,
            tolerance_mm: 0.02,
            bridge_islands: true,
            bridge_width_mm: 1.2,
            min_island_mm2: 6.0,
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
    /// Floating pieces that were connected with bridges.
    pub bridged: usize,
    /// Floating pieces that were too small and were dropped.
    pub dropped: usize,
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
    use i_overlay::core::fill_rule::FillRule;
    use i_overlay::core::overlay_rule::OverlayRule;
    use i_overlay::float::single::SingleFloatOverlay;

    let (w, h) = (seg.width, seg.height);
    let f = (opts.frame_mm * px_per_mm).round().max(0.0) as u32;
    let (pw, ph) = (w + 2 * f, h + 2 * f);
    let idx = |x: u32, y: u32| (y * pw + x) as usize;

    // Solid material: the frame, plus every pixel that is *not* this layer's color.
    let mut solid = vec![true; (pw * ph) as usize];
    for y in 0..h {
        for x in 0..w {
            solid[idx(x + f, y + f)] = seg.region_color[seg.labels[(y * w + x) as usize] as usize] != color;
        }
    }

    // Decide bridges / drops on a scratch copy (painting bridges keeps connectivity right
    // across passes), then remove dropped pieces from the real mask.
    let mut bridges: Vec<[u32; 4]> = Vec::new();
    let (bridged, dropped) = if opts.bridge_islands {
        let mut scratch = solid.clone();
        let bridge_px = (opts.bridge_width_mm * px_per_mm).round() as u32;
        let min_px = (opts.min_island_mm2 * px_per_mm * px_per_mm).round() as u64;
        // Pieces of 100 mm² or more are always worth keeping.
        let always_px = (100.0 * px_per_mm * px_per_mm).round() as u64;
        let rules = IslandRules {
            has_frame: f > 0,
            bridge_px,
            min_px,
            always_bridge_px: always_px,
        };
        let r = resolve_islands(&mut scratch, pw, ph, &rules, &mut bridges);
        for (s, &k) in solid.iter_mut().zip(&scratch) {
            *s &= k; // bridging only adds material; anything removed was dropped
        }
        r
    } else {
        (0, 0)
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

    // Smooth outlines of the material, as one even-odd subject.
    let subject: Vec<Vec<[f64; 2]>> = trace_mask(pw, ph, |x, y| solid[idx(x, y)])
        .iter()
        .map(|l| {
            let pts: Vec<Pt> = l.corners.iter().map(|&(x, y)| to_mm(x, y)).collect();
            simplify_closed(&pts, opts.tolerance_mm)
                .iter()
                .map(|p| [p[0] as f64, p[1] as f64])
                .collect()
        })
        .collect();
    let s = 1.0 / px_per_mm as f64;
    let clip: Vec<Vec<[f64; 2]>> = bridges
        .iter()
        .map(|&[x0, y0, x1, y1]| {
            let (x0, y0, x1, y1) = (x0 as f64 * s, y0 as f64 * s, x1 as f64 * s, y1 as f64 * s);
            vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
        })
        .collect();

    // Robust union: merges bridges and resolves any self-intersections left by smoothing.
    let shapes = subject.overlay(&clip, OverlayRule::Union, FillRule::EvenOdd);
    let ring = |c: &Vec<[f64; 2]>| -> Vec<Pt> { c.iter().map(|p| [p[0] as f32, p[1] as f32]).collect() };
    // Drop microscopic slivers the union can leave behind (< 0.02 mm²).
    const MIN_RING_MM2: f32 = 0.02;
    let big = |r: &Vec<Pt>| r.len() >= 3 && crate::vector::signed_area(r).abs() >= MIN_RING_MM2;
    let mut pieces: Vec<SolidPolygon> = shapes
        .iter()
        .filter(|sh| !sh.is_empty())
        .map(|sh| SolidPolygon {
            outer: ring(&sh[0]),
            holes: sh[1..].iter().map(ring).filter(big).collect(),
        })
        .filter(|p| big(&p.outer))
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
        bridged,
        dropped,
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
        let no_bridges = StencilOptions {
            bridge_islands: false,
            ..Default::default()
        };
        let layers = build_layers(&seg, &sb, &pal, 2.0, &no_bridges);
        assert_eq!(layers.len(), 2);
        let black = layers.iter().find(|l| l.name == "Black").unwrap();
        assert_eq!(black.number, 2); // white is painted first
        assert_eq!(black.island_count(), 1);
        // main sheet: frame outline, 1 ring hole, 4 registration holes
        assert_eq!(black.pieces[0].holes.len(), 5);
        assert!((black.sheet_mm.0 - (30.0 + 24.0)).abs() < 0.6);

        // With bridging the centre is held by two bridges, which split the ring hole in two.
        let layers = build_layers(&seg, &sb, &pal, 2.0, &StencilOptions::default());
        let black = layers.iter().find(|l| l.name == "Black").unwrap();
        assert_eq!(black.island_count(), 0);
        assert_eq!((black.bridged, black.dropped), (1, 0));
        assert_eq!(black.pieces[0].holes.len(), 2 + 4);

        // A tiny centre is dropped instead.
        let tiny = StencilOptions {
            min_island_mm2: 200.0,
            ..Default::default()
        };
        let layers = build_layers(&seg, &sb, &pal, 2.0, &tiny);
        let black = layers.iter().find(|l| l.name == "Black").unwrap();
        assert_eq!((black.island_count(), black.bridged, black.dropped), (0, 0, 1));
        assert_eq!(black.pieces[0].holes.len(), 1 + 4);
    }
}
