//! User palettes: the physical colors (paints, pens, pencils) a user owns.

use crate::color::{Lab, Rgb8};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Swatch {
    pub name: String,
    pub color: Rgb8,
}

impl Swatch {
    pub fn new(name: impl Into<String>, color: Rgb8) -> Self {
        Self {
            name: name.into(),
            color,
        }
    }

    pub fn lab(&self) -> Lab {
        self.color.to_lab()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Palette {
    pub name: String,
    pub swatches: Vec<Swatch>,
}

impl Palette {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            swatches: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.swatches.len()
    }

    pub fn is_empty(&self) -> bool {
        self.swatches.is_empty()
    }

    pub fn load(path: &Path) -> crate::Result<Self> {
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    }

    pub fn save(&self, path: &Path) -> crate::Result<()> {
        std::fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    /// A typical 12-tube student acrylic set. Used as the starter palette.
    pub fn basic_acrylic_12() -> Self {
        let s = |n: &str, h: &str| Swatch::new(n, Rgb8::from_hex(h).unwrap());
        Self {
            name: "Basic 12-Color Acrylic".into(),
            swatches: vec![
                s("Titanium White", "#F4F4F0"),
                s("Mars Black", "#1E1E1E"),
                s("Cadmium Yellow", "#F6C800"),
                s("Yellow Ochre", "#C99A2E"),
                s("Cadmium Orange", "#EE7A1C"),
                s("Cadmium Red", "#C8231E"),
                s("Alizarin Crimson", "#8E1B2C"),
                s("Burnt Sienna", "#8A4A26"),
                s("Burnt Umber", "#4E3324"),
                s("Ultramarine Blue", "#2A3A9A"),
                s("Cerulean Blue", "#2C86C6"),
                s("Sap Green", "#3E6B2A"),
            ],
        }
    }

    /// Black + white only. Classic single-layer stencil.
    pub fn black_and_white() -> Self {
        Self {
            name: "Black & White".into(),
            swatches: vec![Swatch::new("White", Rgb8::WHITE), Swatch::new("Black", Rgb8::BLACK)],
        }
    }

    /// Five-step grayscale, handy for multi-layer value stencils.
    pub fn grayscale_5() -> Self {
        Self {
            name: "Grayscale (5 values)".into(),
            swatches: [255u8, 190, 128, 64, 0]
                .iter()
                .enumerate()
                .map(|(i, &v)| Swatch::new(format!("Value {}", i + 1), Rgb8::new(v, v, v)))
                .collect(),
        }
    }

    pub fn builtins() -> Vec<Palette> {
        vec![Self::basic_acrylic_12(), Self::black_and_white(), Self::grayscale_5()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_roundtrip() {
        let p = Palette::basic_acrylic_12();
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Palette>(&json).unwrap(), p);
    }
}
