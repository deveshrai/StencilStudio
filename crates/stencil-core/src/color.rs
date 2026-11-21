//! Color types and perceptual (CIELAB) color math.

use serde::{Deserialize, Serialize};

/// 8-bit sRGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgb8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb8 {
    pub const WHITE: Rgb8 = Rgb8::new(255, 255, 255);
    pub const BLACK: Rgb8 = Rgb8::new(0, 0, 0);

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Parse `#rrggbb`, `rrggbb`, `#rgb` or `rgb`.
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('#');
        let expand = |c: u8| c * 17;
        match s.len() {
            6 => {
                let v = u32::from_str_radix(s, 16).ok()?;
                Some(Self::new((v >> 16) as u8, (v >> 8) as u8, v as u8))
            }
            3 => {
                let v = u16::from_str_radix(s, 16).ok()?;
                Some(Self::new(
                    expand(((v >> 8) & 0xF) as u8),
                    expand(((v >> 4) & 0xF) as u8),
                    expand((v & 0xF) as u8),
                ))
            }
            _ => None,
        }
    }

    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    pub fn to_lab(self) -> Lab {
        Lab::from_rgb(self)
    }
}

/// CIELAB color (D65 white point).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Lab {
    pub l: f32,
    pub a: f32,
    pub b: f32,
}

fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let v = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (v * 255.0).round() as u8
}

const XN: f32 = 0.950_47;
const YN: f32 = 1.0;
const ZN: f32 = 1.088_83;
const EPS: f32 = 216.0 / 24389.0;
const KAPPA: f32 = 24389.0 / 27.0;

fn f_lab(t: f32) -> f32 {
    if t > EPS { t.cbrt() } else { (KAPPA * t + 16.0) / 116.0 }
}

fn f_lab_inv(t: f32) -> f32 {
    let t3 = t * t * t;
    if t3 > EPS { t3 } else { (116.0 * t - 16.0) / KAPPA }
}

impl Lab {
    pub const fn new(l: f32, a: f32, b: f32) -> Self {
        Self { l, a, b }
    }

    pub fn from_rgb(c: Rgb8) -> Self {
        let (r, g, b) = (srgb_to_linear(c.r), srgb_to_linear(c.g), srgb_to_linear(c.b));
        let x = 0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b;
        let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b;
        let z = 0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b;
        let (fx, fy, fz) = (f_lab(x / XN), f_lab(y / YN), f_lab(z / ZN));
        Lab {
            l: 116.0 * fy - 16.0,
            a: 500.0 * (fx - fy),
            b: 200.0 * (fy - fz),
        }
    }

    /// Linear-light sRGB components, not clamped (values outside 0..1 are out of gamut).
    pub fn to_linear_rgb(self) -> [f32; 3] {
        let fy = (self.l + 16.0) / 116.0;
        let fx = fy + self.a / 500.0;
        let fz = fy - self.b / 200.0;
        let (x, y, z) = (f_lab_inv(fx) * XN, f_lab_inv(fy) * YN, f_lab_inv(fz) * ZN);
        [
            3.240_454_2 * x - 1.537_138_5 * y - 0.498_531_4 * z,
            -0.969_266 * x + 1.876_010_8 * y + 0.041_556 * z,
            0.055_643_4 * x - 0.204_025_9 * y + 1.057_225_2 * z,
        ]
    }

    /// True if this color can be shown/printed as sRGB without clipping.
    pub fn in_srgb_gamut(self) -> bool {
        self.to_linear_rgb().iter().all(|&c| (-1e-4..=1.0001).contains(&c))
    }

    /// Build from lightness, chroma and hue angle (degrees).
    pub fn from_lch(l: f32, c: f32, h_deg: f32) -> Self {
        let h = h_deg.to_radians();
        Lab::new(l, c * h.cos(), c * h.sin())
    }

    pub fn to_rgb(self) -> Rgb8 {
        let [r, g, b] = self.to_linear_rgb();
        Rgb8::new(linear_to_srgb(r), linear_to_srgb(g), linear_to_srgb(b))
    }

    /// Squared CIE76 distance (cheap; good for k-means).
    pub fn dist2(self, o: Lab) -> f32 {
        let (dl, da, db) = (self.l - o.l, self.a - o.a, self.b - o.b);
        dl * dl + da * da + db * db
    }

    /// CIEDE2000 color difference.
    pub fn delta_e2000(self, o: Lab) -> f32 {
        use std::f32::consts::PI;
        let (l1, a1, b1) = (self.l, self.a, self.b);
        let (l2, a2, b2) = (o.l, o.a, o.b);
        let c1 = (a1 * a1 + b1 * b1).sqrt();
        let c2 = (a2 * a2 + b2 * b2).sqrt();
        let cbar = (c1 + c2) / 2.0;
        let cbar7 = cbar.powi(7);
        let g = 0.5 * (1.0 - (cbar7 / (cbar7 + 25f32.powi(7))).sqrt());
        let a1p = (1.0 + g) * a1;
        let a2p = (1.0 + g) * a2;
        let c1p = (a1p * a1p + b1 * b1).sqrt();
        let c2p = (a2p * a2p + b2 * b2).sqrt();
        let hue = |b: f32, a: f32| {
            if a == 0.0 && b == 0.0 {
                0.0
            } else {
                let h = b.atan2(a);
                if h < 0.0 { h + 2.0 * PI } else { h }
            }
        };
        let h1p = hue(b1, a1p);
        let h2p = hue(b2, a2p);
        let dlp = l2 - l1;
        let dcp = c2p - c1p;
        let dhp = if c1p * c2p == 0.0 {
            0.0
        } else {
            let d = h2p - h1p;
            if d.abs() <= PI {
                d
            } else if d > PI {
                d - 2.0 * PI
            } else {
                d + 2.0 * PI
            }
        };
        let dhp_big = 2.0 * (c1p * c2p).sqrt() * (dhp / 2.0).sin();
        let lbarp = (l1 + l2) / 2.0;
        let cbarp = (c1p + c2p) / 2.0;
        let hbarp = if c1p * c2p == 0.0 {
            h1p + h2p
        } else if (h1p - h2p).abs() <= PI {
            (h1p + h2p) / 2.0
        } else if h1p + h2p < 2.0 * PI {
            (h1p + h2p + 2.0 * PI) / 2.0
        } else {
            (h1p + h2p - 2.0 * PI) / 2.0
        };
        let t =
            1.0 - 0.17 * (hbarp - PI / 6.0).cos() + 0.24 * (2.0 * hbarp).cos() + 0.32 * (3.0 * hbarp + PI / 30.0).cos()
                - 0.20 * (4.0 * hbarp - 63.0 * PI / 180.0).cos();
        let dtheta = (30.0 * PI / 180.0) * (-((hbarp * 180.0 / PI - 275.0) / 25.0).powi(2)).exp();
        let cbarp7 = cbarp.powi(7);
        let rc = 2.0 * (cbarp7 / (cbarp7 + 25f32.powi(7))).sqrt();
        let lb50 = (lbarp - 50.0).powi(2);
        let sl = 1.0 + 0.015 * lb50 / (20.0 + lb50).sqrt();
        let sc = 1.0 + 0.045 * cbarp;
        let sh = 1.0 + 0.015 * cbarp * t;
        let rt = -(2.0 * dtheta).sin() * rc;
        let (tl, tc, th) = (dlp / sl, dcp / sc, dhp_big / sh);
        (tl * tl + tc * tc + th * th + rt * tc * th).max(0.0).sqrt()
    }
}

/// Which color difference formula to use when mapping pixels to palette colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DistanceMetric {
    /// Euclidean distance in CIELAB. Fast.
    Cie76,
    /// CIEDE2000. Most perceptually accurate.
    #[default]
    Ciede2000,
}

impl DistanceMetric {
    pub fn distance(self, a: Lab, b: Lab) -> f32 {
        match self {
            DistanceMetric::Cie76 => a.dist2(b).sqrt(),
            DistanceMetric::Ciede2000 => a.delta_e2000(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let c = Rgb8::from_hex("#1a2B3c").unwrap();
        assert_eq!(c, Rgb8::new(0x1a, 0x2b, 0x3c));
        assert_eq!(c.to_hex(), "#1A2B3C");
        assert_eq!(Rgb8::from_hex("fff"), Some(Rgb8::WHITE));
        assert_eq!(Rgb8::from_hex("nope"), None);
    }

    #[test]
    fn lab_known_values() {
        let w = Rgb8::WHITE.to_lab();
        assert!(
            (w.l - 100.0).abs() < 0.05 && w.a.abs() < 0.05 && w.b.abs() < 0.05,
            "{w:?}"
        );
        let r = Rgb8::new(255, 0, 0).to_lab();
        assert!(
            (r.l - 53.24).abs() < 0.1 && (r.a - 80.09).abs() < 0.2 && (r.b - 67.20).abs() < 0.2,
            "{r:?}"
        );
    }

    #[test]
    fn lab_rgb_roundtrip() {
        for c in [Rgb8::new(12, 200, 99), Rgb8::new(255, 128, 0), Rgb8::new(3, 3, 3)] {
            assert_eq!(c.to_lab().to_rgb(), c);
        }
    }

    #[test]
    fn ciede2000_reference_pair() {
        // Sharma et al. test pair 1: expected 2.0425
        let a = Lab::new(50.0, 2.6772, -79.7751);
        let b = Lab::new(50.0, 0.0, -82.7485);
        assert!((a.delta_e2000(b) - 2.0425).abs() < 1e-3);
    }
}
