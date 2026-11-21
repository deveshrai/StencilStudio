//! File writers for stencil layers: SVG and DXF (laser / cutter), STL (3D print).

use crate::stencil::{StencilLayer, StencilOptions};
use crate::vector::Pt;
use std::fmt::Write as _;

fn path_data(paths: &[&Vec<Pt>]) -> String {
    let mut d = String::new();
    for p in paths {
        for (i, q) in p.iter().enumerate() {
            let _ = write!(d, "{}{:.3},{:.3} ", if i == 0 { "M" } else { "L" }, q[0], q[1]);
        }
        d.push_str("Z ");
    }
    d
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Laser-ready SVG in millimetres: red hairline cut paths, blue engraved label.
pub fn layer_svg(layer: &StencilLayer, opts: &StencilOptions) -> String {
    let (w, h) = layer.sheet_mm;
    let paths: Vec<&Vec<Pt>> = layer.cut_paths().collect();
    let mut s = String::new();
    let _ = writeln!(s, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = writeln!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:inkscape="http://www.inkscape.org/namespaces/inkscape" width="{w:.3}mm" height="{h:.3}mm" viewBox="0 0 {w:.3} {h:.3}">"#
    );
    let _ = writeln!(s, "  <title>{}</title>", xml_escape(&layer.label()));
    let _ = writeln!(
        s,
        r##"  <g id="cut" inkscape:groupmode="layer" inkscape:label="Cut" fill="none" stroke="#FF0000" stroke-width="0.01">"##
    );
    let _ = writeln!(
        s,
        r#"    <path fill-rule="evenodd" d="{}"/>"#,
        path_data(&paths).trim_end()
    );
    let _ = writeln!(s, "  </g>");
    if let Some((p, size)) = layer.label_anchor(opts.frame_mm) {
        let _ = writeln!(
            s,
            r##"  <g id="engrave" inkscape:groupmode="layer" inkscape:label="Engrave" fill="#0000FF" stroke="none">"##
        );
        let _ = writeln!(
            s,
            r#"    <text x="{:.3}" y="{:.3}" font-family="Helvetica, Arial, sans-serif" font-size="{size:.2}">{}</text>"#,
            p[0],
            p[1],
            xml_escape(&layer.label())
        );
        let _ = writeln!(s, "  </g>");
    }
    s.push_str("</svg>\n");
    s
}

/// AutoCAD R12 ASCII DXF (the most widely supported flavour), units = mm.
/// Layers: CUT (closed polylines) and ENGRAVE (label text). Y axis points up.
pub fn layer_dxf(layer: &StencilLayer, opts: &StencilOptions) -> String {
    let h = layer.sheet_mm.1;
    let mut s = String::new();
    let mut pair = |code: i32, v: &str| {
        let _ = writeln!(s, "{code}\n{v}");
    };
    pair(0, "SECTION");
    pair(2, "HEADER");
    pair(9, "$ACADVER");
    pair(1, "AC1009");
    pair(9, "$INSUNITS");
    pair(70, "4");
    pair(0, "ENDSEC");
    pair(0, "SECTION");
    pair(2, "TABLES");
    pair(0, "TABLE");
    pair(2, "LAYER");
    pair(70, "2");
    for (name, color) in [("CUT", "1"), ("ENGRAVE", "5")] {
        pair(0, "LAYER");
        pair(2, name);
        pair(70, "0");
        pair(62, color);
        pair(6, "CONTINUOUS");
    }
    pair(0, "ENDTAB");
    pair(0, "ENDSEC");
    pair(0, "SECTION");
    pair(2, "ENTITIES");
    for path in layer.cut_paths() {
        pair(0, "POLYLINE");
        pair(8, "CUT");
        pair(66, "1");
        pair(70, "1");
        pair(10, "0.0");
        pair(20, "0.0");
        pair(30, "0.0");
        for p in path {
            pair(0, "VERTEX");
            pair(8, "CUT");
            pair(10, &format!("{:.4}", p[0]));
            pair(20, &format!("{:.4}", h - p[1]));
            pair(30, "0.0");
        }
        pair(0, "SEQEND");
        pair(8, "CUT");
    }
    if let Some((p, size)) = layer.label_anchor(opts.frame_mm) {
        let text: String = layer
            .label()
            .chars()
            .map(|c| if c.is_ascii() { c } else { '?' })
            .collect();
        pair(0, "TEXT");
        pair(8, "ENGRAVE");
        pair(10, &format!("{:.4}", p[0]));
        pair(20, &format!("{:.4}", h - p[1]));
        pair(30, "0.0");
        pair(40, &format!("{size:.3}"));
        pair(1, &text);
    }
    pair(0, "ENDSEC");
    pair(0, "EOF");
    s
}

/// Binary STL of the extruded stencil sheet (mm). Y is flipped so the model is not
/// mirrored when viewed from above in a slicer.
pub fn layer_stl(layer: &StencilLayer, opts: &StencilOptions) -> Vec<u8> {
    let t = opts.thickness_mm.max(0.1);
    let h = layer.sheet_mm.1;
    let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
    let mut earcut = earcut::Earcut::new();
    let mut idx: Vec<u32> = Vec::new();

    // Flip to y-up. Outer loops come out counter-clockwise, holes clockwise.
    let flip = |ring: &Vec<Pt>| -> Vec<[f64; 2]> {
        // Snap to a 1 µm grid first so nearly-coincident points from different rings
        // become identical (and are then separated as pinches), then jitter.
        let snap = |v: f64| (v * 1000.0).round() / 1000.0;
        let mut v: Vec<[f64; 2]> = ring
            .iter()
            .map(|p| jitter([snap(p[0] as f64), snap((h - p[1]) as f64)]))
            .collect();
        v.dedup();
        while v.len() > 1 && v.first() == v.last() {
            v.pop();
        }
        v
    };
    let mut all: Vec<Vec<Vec<[f64; 2]>>> = Vec::new();
    for piece in &layer.pieces {
        let mut rings: Vec<Vec<[f64; 2]>> = vec![orient(flip(&piece.outer), true)];
        if rings[0].len() < 3 {
            continue;
        }
        rings.extend(
            piece
                .holes
                .iter()
                .map(|r| orient(flip(r), false))
                .filter(|r| r.len() >= 3),
        );
        all.push(rings);
    }
    separate_pinches(&mut all);

    for rings in &all {
        let mut verts: Vec<[f64; 2]> = Vec::new();
        let mut hole_starts: Vec<u32> = Vec::new();
        for (i, r) in rings.iter().enumerate() {
            if i > 0 {
                hole_starts.push(verts.len() as u32);
            }
            verts.extend_from_slice(r);
        }
        earcut.earcut(verts.iter().copied(), &hole_starts, &mut idx);
        for tri in idx.as_chunks::<3>().0 {
            let [a, b, c] = [verts[tri[0] as usize], verts[tri[1] as usize], verts[tri[2] as usize]];
            let ccw = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]) > 0.0;
            let (b, c) = if ccw { (b, c) } else { (c, b) };
            let p = |q: [f64; 2], z: f32| [q[0] as f32, q[1] as f32, z];
            tris.push([p(a, t), p(b, t), p(c, t)]); // top, facing +z
            tris.push([p(a, 0.0), p(c, 0.0), p(b, 0.0)]); // bottom, facing -z
        }
        // Side walls: outer CCW / holes CW => the material is on the left of each edge.
        for r in rings {
            for i in 0..r.len() {
                let (a, b) = (r[i], r[(i + 1) % r.len()]);
                let a0 = [a[0] as f32, a[1] as f32, 0.0];
                let b0 = [b[0] as f32, b[1] as f32, 0.0];
                let a1 = [a[0] as f32, a[1] as f32, t];
                let b1 = [b[0] as f32, b[1] as f32, t];
                tris.push([a0, b0, b1]);
                tris.push([a0, b1, a1]);
            }
        }
    }

    let mut out = Vec::with_capacity(84 + tris.len() * 50);
    let mut header = [0u8; 80];
    let title = format!("StencilStudio {}", layer.label());
    for (d, s) in header.iter_mut().zip(title.bytes().filter(u8::is_ascii)) {
        *d = s;
    }
    out.extend_from_slice(&header);
    out.extend_from_slice(&(tris.len() as u32).to_le_bytes());
    for t in &tris {
        let n = normal(t);
        for v in std::iter::once(n).chain(t.iter().copied()) {
            for c in v {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        out.extend_from_slice(&0u16.to_le_bytes());
    }
    out
}

/// Deterministic sub-micron offset derived from the point itself. Breaks exact
/// collinearity between different rings (e.g. cut-outs touching the same image
/// edge), which otherwise makes ear clipping emit zero-area slivers and
/// T-junctions. Identical input points get identical offsets, so shared points stay shared.
fn jitter(p: [f64; 2]) -> [f64; 2] {
    let mut x = p[0].to_bits() ^ p[1].to_bits().rotate_left(29) ^ 0x9E37_79B9_7F4A_7C15;
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    let u = |b: u64| ((b & 0xFFFF) as f64 / 65535.0 - 0.5) * 1e-3;
    [p[0] + u(x), p[1] + u(x >> 16)]
}

/// Where two loops touch at a single point (diagonal pixel saddles), nudge each
/// occurrence 10 µm towards its own loop's interior so the mesh is strictly manifold.
fn separate_pinches(pieces: &mut [Vec<Vec<[f64; 2]>>]) {
    use std::collections::HashMap;
    let key = |p: [f64; 2]| (p[0].to_bits(), p[1].to_bits());
    let mut count: HashMap<(u64, u64), u32> = HashMap::new();
    for rings in pieces.iter() {
        for r in rings {
            for &p in r {
                *count.entry(key(p)).or_default() += 1;
            }
        }
    }
    const EPS: f64 = 0.01;
    for rings in pieces.iter_mut() {
        for r in rings.iter_mut() {
            let n = r.len();
            let orig = r.clone();
            for i in 0..n {
                if count[&key(orig[i])] < 2 {
                    continue;
                }
                let (a, b) = (orig[(i + n - 1) % n], orig[(i + 1) % n]);
                let m = [(a[0] + b[0]) / 2.0 - orig[i][0], (a[1] + b[1]) / 2.0 - orig[i][1]];
                let len = (m[0] * m[0] + m[1] * m[1]).sqrt();
                if len > 1e-9 {
                    r[i] = [orig[i][0] + m[0] / len * EPS, orig[i][1] + m[1] / len * EPS];
                }
            }
        }
    }
}

fn orient(mut ring: Vec<[f64; 2]>, ccw: bool) -> Vec<[f64; 2]> {
    let n = ring.len();
    let area: f64 = (0..n)
        .map(|i| ring[i][0] * ring[(i + 1) % n][1] - ring[(i + 1) % n][0] * ring[i][1])
        .sum();
    if (area > 0.0) != ccw {
        ring.reverse();
    }
    ring
}

fn normal(t: &[[f32; 3]; 3]) -> [f32; 3] {
    let u = [t[1][0] - t[0][0], t[1][1] - t[0][1], t[1][2] - t[0][2]];
    let v = [t[2][0] - t[0][0], t[2][1] - t[0][1], t[2][2] - t[0][2]];
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-12);
    [n[0] / len, n[1] / len, n[2] / len]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgb8;
    use crate::stencil::SolidPolygon;
    use std::collections::HashMap;

    fn square_with_hole() -> StencilLayer {
        StencilLayer {
            number: 1,
            palette_index: 0,
            name: "Red & Co".into(),
            color: Rgb8::new(200, 0, 0),
            sheet_mm: (100.0, 80.0),
            bridged: 0,
            dropped: 0,
            pieces: vec![SolidPolygon {
                outer: vec![[0.0, 0.0], [100.0, 0.0], [100.0, 80.0], [0.0, 80.0]],
                holes: vec![vec![[30.0, 30.0], [30.0, 50.0], [60.0, 50.0], [60.0, 30.0]]],
            }],
        }
    }

    #[test]
    fn stl_is_watertight_with_correct_volume() {
        let opts = StencilOptions {
            thickness_mm: 2.0,
            ..Default::default()
        };
        let stl = layer_stl(&square_with_hole(), &opts);
        let n = u32::from_le_bytes(stl[80..84].try_into().unwrap()) as usize;
        assert_eq!(stl.len(), 84 + n * 50);
        let mut tris = Vec::new();
        for i in 0..n {
            let base = 84 + i * 50 + 12;
            let f = |k: usize| f32::from_le_bytes(stl[base + k * 4..base + k * 4 + 4].try_into().unwrap());
            tris.push([[f(0), f(1), f(2)], [f(3), f(4), f(5)], [f(6), f(7), f(8)]]);
        }
        // Every directed edge must be matched by its reverse exactly once (closed 2-manifold).
        let key = |p: [f32; 3]| (p[0].to_bits(), p[1].to_bits(), p[2].to_bits());
        let mut edges: HashMap<_, i32> = HashMap::new();
        for t in &tris {
            for i in 0..3 {
                *edges.entry((key(t[i]), key(t[(i + 1) % 3]))).or_default() += 1;
            }
        }
        for ((a, b), c) in &edges {
            assert_eq!(*c, 1);
            assert_eq!(edges.get(&(*b, *a)), Some(&1), "unmatched edge");
        }
        // Signed volume via divergence theorem: (100*80 - 30*20) * 2
        let vol: f32 = tris
            .iter()
            .map(|t| {
                let [a, b, c] = *t;
                (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                    + a[2] * (b[0] * c[1] - b[1] * c[0]))
                    / 6.0
            })
            .sum();
        assert!((vol - 14800.0).abs() < 1.0, "volume {vol}");
    }

    #[test]
    fn svg_and_dxf_contain_paths() {
        let l = square_with_hole();
        let svg = layer_svg(&l, &StencilOptions::default());
        assert!(svg.contains("width=\"100.000mm\"") && svg.contains("Red &amp; Co"));
        assert_eq!(svg.matches('Z').count(), 2);
        let dxf = layer_dxf(&l, &StencilOptions::default());
        assert_eq!(dxf.matches("POLYLINE").count(), 2);
        assert!(dxf.trim_end().ends_with("EOF"));
    }
}
