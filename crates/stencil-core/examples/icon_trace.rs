//! Dev tool: trace a flat-colour raster logo into a clean SVG with the engine's own
//! smooth shared-edge vectoriser (used for packaging/icon/hummingbird.svg).
//!
//! `cargo run -p stencil-core --release --example icon_trace -- <in> <out.svg> <bg-hex> <hex>...`
//!
//! Every pixel snaps to the nearest listed colour (CIEDE2000), a majority filter removes
//! anti-aliasing fringes, specks are merged away and the shared edges are smoothed. The
//! first colour is the background: it is not emitted, so the pieces sit on whatever
//! backdrop the icon uses.
use stencil_core::regions::Segmentation;
use stencil_core::vector::{SmoothBoundaries, color_contours};
use stencil_core::{Rgb8, color::Lab};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 5 {
        eprintln!("usage: icon_trace <in> <out.svg> <bg-hex> <hex>...");
        std::process::exit(2);
    }
    let img = image::open(&a[1]).expect("open").to_rgb8();
    let img = image::imageops::blur(&img, 1.2);
    let (w, h) = img.dimensions();
    let cols: Vec<Rgb8> = a[3..].iter().map(|s| Rgb8::from_hex(s).expect("hex")).collect();
    let labs: Vec<Lab> = cols.iter().map(|c| c.to_lab()).collect();

    let mut map: Vec<u16> = img
        .pixels()
        .map(|p| {
            let l = Rgb8::new(p[0], p[1], p[2]).to_lab();
            (0..labs.len())
                .min_by(|&i, &j| l.delta_e2000(labs[i]).total_cmp(&l.delta_e2000(labs[j])))
                .unwrap() as u16
        })
        .collect();

    // Morphological opening per piece colour: anti-aliasing fringes (e.g. the brownish
    // ring where orange meets the dark backdrop) are only a few pixels wide and vanish,
    // while real pieces and the dark gaps between them survive. Each removed pixel goes
    // to whichever nearby surviving colour (or the backdrop) it is closest to.
    let n = labs.len();
    let r = 2;
    let mut opened: Vec<Vec<bool>> = vec![vec![false; (w * h) as usize]; n];
    let mut near: Vec<Vec<bool>> = vec![vec![false; (w * h) as usize]; n];
    for c in 1..n {
        let m: Vec<bool> = map.iter().map(|&v| v as usize == c).collect();
        let er = morph(&m, w, h, r, false);
        opened[c] = morph(&er, w, h, r, true);
        near[c] = morph(&opened[c], w, h, r + 3, true);
    }
    let src = img.clone();
    for i in 0..(w * h) as usize {
        let c = map[i] as usize;
        if c == 0 || opened[c][i] {
            continue;
        }
        let p = src.as_raw();
        let l = Rgb8::new(p[i * 3], p[i * 3 + 1], p[i * 3 + 2]).to_lab();
        map[i] = (0..n)
            .filter(|&k| k == 0 || near[k][i])
            .min_by(|&a, &b| l.delta_e2000(labs[a]).total_cmp(&l.delta_e2000(labs[b])))
            .unwrap() as u16;
    }

    let seg = Segmentation::from_color_map(w, h, &map).merge_small_regions(60);
    let sb = SmoothBoundaries::build(&seg, 2.2);
    let mut out =
        format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w} {h}\" width=\"{w}\" height=\"{h}\">\n");
    let mut bbox = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for (ci, c) in cols.iter().enumerate().skip(1) {
        let loops = color_contours(&seg, &sb, ci as u16, 0.2);
        if loops.is_empty() {
            continue;
        }
        let mut d = String::new();
        for l in &loops {
            for (i, p) in l.iter().enumerate() {
                bbox = [
                    bbox[0].min(p[0]),
                    bbox[1].min(p[1]),
                    bbox[2].max(p[0]),
                    bbox[3].max(p[1]),
                ];
                d.push_str(&format!("{}{:.1} {:.1}", if i == 0 { "M" } else { "L" }, p[0], p[1]));
            }
            d.push('Z');
        }
        out.push_str(&format!(
            "<path fill=\"{}\" fill-rule=\"evenodd\" d=\"{d}\"/>\n",
            c.to_hex()
        ));
    }
    out.push_str("</svg>\n");
    std::fs::write(&a[2], out).expect("write");
    eprintln!("bbox {:?}, {} regions", bbox, seg.region_count());
}

/// Square erosion (`dilate = false`) or dilation of a mask, separable.
fn morph(m: &[bool], w: u32, h: u32, r: i32, dilate: bool) -> Vec<bool> {
    let (w, h) = (w as i32, h as i32);
    let pass = |src: &[bool], horiz: bool| -> Vec<bool> {
        let mut out = vec![false; src.len()];
        for y in 0..h {
            for x in 0..w {
                let mut acc = !dilate;
                for d in -r..=r {
                    let (xx, yy) = if horiz { (x + d, y) } else { (x, y + d) };
                    let v = if xx < 0 || yy < 0 || xx >= w || yy >= h {
                        false
                    } else {
                        src[(yy * w + xx) as usize]
                    };
                    if dilate { acc |= v } else { acc &= v }
                }
                out[(y * w + x) as usize] = acc;
            }
        }
        out
    };
    let a = pass(m, true);
    pass(&a, false)
}
