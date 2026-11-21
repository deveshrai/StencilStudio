//! Pull the scanned picture out of a PDF (what scanner apps and printers' "scan to
//! PDF" produce): finds every image XObject and decodes the largest one.
//!
//! Supported: JPEG (`/DCTDecode`) and deflate (`/FlateDecode`, with or without PNG
//! predictors) in gray, RGB or CMYK, 8 bits per component. That covers scanner output;
//! JPEG 2000 and other exotic encodings return an error asking for a JPG/PNG instead.

use image::{DynamicImage, RgbImage};

pub fn largest_image(pdf: &[u8]) -> Result<RgbImage, String> {
    let mut best: Option<(u64, RgbImage)> = None;
    let mut unsupported = None;
    let mut pos = 0;
    while let Some(off) = find(&pdf[pos..], b"stream") {
        let s = pos + off;
        pos = s + 6;
        if pdf[..s].ends_with(b"end") {
            continue; // "endstream"
        }
        // the stream's dictionary is the last "<<" ... ">>" before the keyword
        let Some(dict_start) = rfind(&pdf[..s], b"obj") else {
            continue;
        };
        let dict = &pdf[dict_start..s];
        if find(dict, b"/Image").is_none() {
            continue;
        }
        let mut data_start = s + 6;
        if pdf.get(data_start) == Some(&b'\r') {
            data_start += 1;
        }
        if pdf.get(data_start) == Some(&b'\n') {
            data_start += 1;
        }
        let Some(end) = find(&pdf[data_start..], b"endstream") else {
            continue;
        };
        let mut data = &pdf[data_start..data_start + end];
        while let Some((&last, rest)) = data.split_last() {
            if last == b'\n' || last == b'\r' {
                data = rest;
            } else {
                break;
            }
        }
        let d = String::from_utf8_lossy(dict);
        let w = number_after(&d, "/Width").unwrap_or(0);
        let h = number_after(&d, "/Height").unwrap_or(0);
        let bpc = number_after(&d, "/BitsPerComponent").unwrap_or(8);
        if w == 0 || h == 0 || d.contains("/ImageMask true") {
            continue;
        }
        let img = if d.contains("/DCTDecode") {
            image::load_from_memory_with_format(data, image::ImageFormat::Jpeg)
                .map(|i| i.to_rgb8())
                .map_err(|e| e.to_string())
        } else if d.contains("/FlateDecode") && bpc == 8 {
            let predictor = number_after(&d, "/Predictor").unwrap_or(1);
            inflate(data, w, h, predictor)
        } else {
            unsupported = Some(if d.contains("/JPXDecode") {
                "JPEG 2000"
            } else {
                "this image encoding"
            });
            continue;
        };
        if let Ok(img) = img {
            let px = img.width() as u64 * img.height() as u64;
            if best.as_ref().is_none_or(|b| px > b.0) {
                best = Some((px, img));
            }
        }
    }
    match (best, unsupported) {
        (Some((_, img)), _) => Ok(img),
        (None, Some(kind)) => Err(format!(
            "This PDF stores its picture as {kind}, which StencilStudio can't read yet. Export the scan as JPG or PNG instead."
        )),
        (None, None) => Err("No scanned picture found in this PDF.".into()),
    }
}

fn inflate(data: &[u8], w: u32, h: u32, predictor: u32) -> Result<RgbImage, String> {
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib(data).map_err(|e| format!("{e:?}"))?;
    let px = (w * h) as usize;
    // channel count from the data size (colour spaces are often indirect ICC refs)
    let png_rows = predictor >= 10;
    let channels = [1usize, 3, 4]
        .into_iter()
        .find(|&c| raw.len() == px * c + if png_rows { h as usize } else { 0 })
        .ok_or("unexpected image size")?;
    let row = w as usize * channels;
    let mut out = vec![0u8; px * channels];
    if png_rows {
        let mut prev = vec![0u8; row];
        for y in 0..h as usize {
            let src = &raw[y * (row + 1)..(y + 1) * (row + 1)];
            let (filter, line) = (src[0], &src[1..]);
            let dst = &mut out[y * row..(y + 1) * row];
            for i in 0..row {
                let a = if i >= channels { dst[i - channels] as i32 } else { 0 };
                let b = prev[i] as i32;
                let c = if i >= channels { prev[i - channels] as i32 } else { 0 };
                let v = line[i] as i32;
                dst[i] = match filter {
                    1 => v + a,
                    2 => v + b,
                    3 => v + (a + b) / 2,
                    4 => {
                        let p = a + b - c;
                        let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                        v + if pa <= pb && pa <= pc {
                            a
                        } else if pb <= pc {
                            b
                        } else {
                            c
                        }
                    }
                    _ => v,
                } as u8;
            }
            prev.copy_from_slice(dst);
        }
    } else {
        out.copy_from_slice(&raw);
    }
    Ok(match channels {
        1 => DynamicImage::ImageLuma8(image::GrayImage::from_raw(w, h, out).ok_or("size")?).to_rgb8(),
        3 => RgbImage::from_raw(w, h, out).ok_or("size")?,
        _ => RgbImage::from_fn(w, h, |x, y| {
            let i = ((y * w + x) * 4) as usize;
            let k = 255.0 - out[i + 3] as f32;
            image::Rgb([0, 1, 2].map(|c| ((255.0 - out[i + c] as f32) * k / 255.0) as u8))
        }),
    })
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

fn number_after(d: &str, key: &str) -> Option<u32> {
    let i = d.find(key)? + key.len();
    let rest = d[i..].trim_start();
    let n: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    // an indirect reference ("12 0 R") is not a literal value
    let after = rest[n.len()..].trim_start();
    if after.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    n.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pdf_with_image(dict: &str, data: &[u8]) -> Vec<u8> {
        let mut v = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n5 0 obj\n".to_vec();
        v.extend_from_slice(dict.as_bytes());
        v.extend_from_slice(b"\nstream\n");
        v.extend_from_slice(data);
        v.extend_from_slice(b"\nendstream\nendobj\n%%EOF\n");
        v
    }

    #[test]
    fn reads_flate_rgb_with_png_predictor() {
        // 2x2 RGB, rows filtered with "Up" (2) on the second row
        let rows: Vec<u8> = vec![0, 255, 0, 0, 0, 255, 0, 2, 0, 0, 255, 255, 0, 0];
        let z = miniz_oxide::deflate::compress_to_vec_zlib(&rows, 6);
        let pdf = pdf_with_image(
            &format!(
                "<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace 7 0 R /BitsPerComponent 8 /Filter /FlateDecode /DecodeParms << /Predictor 15 /Colors 3 /Columns 2 >> /Length {} >>",
                z.len()
            ),
            &z,
        );
        let img = largest_image(&pdf).unwrap();
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0]);
        assert_eq!(img.get_pixel(1, 0).0, [0, 255, 0]);
        assert_eq!(img.get_pixel(0, 1).0, [255, 0, 255]); // up: (255,0,0)+(0,0,255)
    }

    #[test]
    fn reads_jpeg() {
        let src = RgbImage::from_pixel(16, 8, image::Rgb([200, 40, 30]));
        let mut jpg = Vec::new();
        DynamicImage::ImageRgb8(src)
            .write_to(&mut std::io::Cursor::new(&mut jpg), image::ImageFormat::Jpeg)
            .unwrap();
        let pdf = pdf_with_image(
            "<< /Subtype /Image /Width 16 /Height 8 /BitsPerComponent 8 /ColorSpace /DeviceRGB /Filter /DCTDecode >>",
            &jpg,
        );
        let img = largest_image(&pdf).unwrap();
        assert_eq!(img.dimensions(), (16, 8));
        assert!((img.get_pixel(4, 4)[0] as i32 - 200).abs() < 8);
    }

    #[test]
    fn no_image_is_an_error() {
        assert!(largest_image(b"%PDF-1.4\n1 0 obj << >> endobj\n").is_err());
    }
}
