//! Subject detection for "Remove background".
//!
//! Best available source first:
//! 1. **Apple Vision** (macOS 14+): `VNGenerateForegroundInstanceMaskRequest`, the same
//!    subject lifting the system uses in Photos and Preview.
//! 2. **Bundled U²-Net-p** (Apache-2.0, 4.6 MB) run on the CPU with `tract`, on every
//!    other platform (Android, older macOS, Windows, Linux).
//! 3. The engine's colour-model segmentation, if both of those fail.
//!
//! The soft mask from 1 or 2 is turned into the final cut-out by
//! [`stencil_core::background::mask_from_probability`], which snaps the edge to the
//! image's own colours at full resolution.

use image::RgbImage;
use std::sync::OnceLock;
use stencil_core::background;

/// Which detector produced the last mask (shown in the status bar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    AppleVision,
    Model,
    Colour,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::AppleVision => "Apple Vision",
            Source::Model => "on-device model",
            Source::Colour => "colour analysis",
        }
    }
}

/// Subject mask (`true` = subject) for `img`, plus which detector produced it.
pub fn foreground_mask(img: &RgbImage) -> (Vec<bool>, Source) {
    #[cfg(target_os = "macos")]
    if std::env::var_os("STENCIL_NO_VISION").is_none()
        && let Some((p, w, h)) = vision::foreground(img)
        && let Some(m) = background::mask_from_probability(img, &p, w, h)
    {
        return (m, Source::AppleVision);
    }
    if let Some((p, w, h)) = model::foreground(img)
        && let Some(m) = background::mask_from_probability(img, &p, w, h)
    {
        return (m, Source::Model);
    }
    (background::foreground_mask(img), Source::Colour)
}

mod model {
    use super::*;
    use tract_onnx::prelude::*;

    const SIDE: usize = 320;
    static MODEL_BYTES: &[u8] = include_bytes!("../models/u2netp.onnx");
    type Plan = std::sync::Arc<TypedSimplePlan>;
    static PLAN: OnceLock<Option<Plan>> = OnceLock::new();

    fn plan() -> Option<&'static Plan> {
        PLAN.get_or_init(|| {
            let r = (|| -> TractResult<Plan> {
                tract_onnx::onnx()
                    .model_for_read(&mut std::io::Cursor::new(MODEL_BYTES))?
                    .with_input_fact(0, f32::fact([1, 3, SIDE, SIDE]).into())?
                    .into_optimized()?
                    .into_runnable()
            })();
            if let Err(e) = &r {
                eprintln!("subject model failed to load: {e:?}");
            }
            r.ok()
        })
        .as_ref()
    }

    /// Soft subject probability at 320×320 (U²-Net's salient-object output, min-max normalised).
    pub fn foreground(img: &RgbImage) -> Option<(Vec<f32>, u32, u32)> {
        let plan = plan()?;
        let small = image::imageops::resize(img, SIDE as u32, SIDE as u32, image::imageops::FilterType::Triangle);
        let max = small.as_raw().iter().copied().max().unwrap_or(255).max(1) as f32;
        let mean = [0.485f32, 0.456, 0.406];
        let std = [0.229f32, 0.224, 0.225];
        let input = tract_ndarray::Array4::from_shape_fn((1, 3, SIDE, SIDE), |(_, c, y, x)| {
            (small.get_pixel(x as u32, y as u32)[c] as f32 / max - mean[c]) / std[c]
        });
        let out = plan.run(tvec!(Tensor::from(input).into())).ok()?;
        let d1 = out[0].to_plain_array_view::<f32>().ok()?;
        let v: Vec<f32> = d1.iter().copied().collect();
        if v.len() != SIDE * SIDE {
            return None;
        }
        let (lo, hi) = v.iter().fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
        if hi - lo < 1e-3 {
            return None;
        }
        Some((
            v.iter().map(|x| (x - lo) / (hi - lo)).collect(),
            SIDE as u32,
            SIDE as u32,
        ))
    }
}

#[cfg(target_os = "macos")]
mod vision {
    use super::*;
    use objc2::AnyThread;
    use objc2::rc::Retained;
    use objc2::runtime::AnyClass;
    use objc2_core_video::{
        CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight, CVPixelBufferGetWidth,
        CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    };
    use objc2_foundation::{NSArray, NSData, NSDictionary};
    use objc2_vision::{VNGenerateForegroundInstanceMaskRequest, VNImageRequestHandler, VNRequest};

    /// Full-resolution subject probability from Apple Vision, or `None` before macOS 14
    /// or when Vision finds no subject.
    pub fn foreground(img: &RgbImage) -> Option<(Vec<f32>, u32, u32)> {
        // The request class only exists on macOS 14+.
        AnyClass::get(c"VNGenerateForegroundInstanceMaskRequest")?;
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(img.clone())
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .ok()?;
        objc2::rc::autoreleasepool(|_| unsafe {
            let data = NSData::with_bytes(&png);
            let opts = NSDictionary::new();
            let handler = VNImageRequestHandler::initWithData_options(VNImageRequestHandler::alloc(), &data, &opts);
            let req = VNGenerateForegroundInstanceMaskRequest::new();
            let as_req: Retained<VNRequest> = Retained::into_super(Retained::into_super(req.clone()));
            handler
                .performRequests_error(&NSArray::from_retained_slice(&[as_req]))
                .ok()?;
            let obs = req.results()?.firstObject()?;
            let all = obs.allInstances();
            let pb = obs
                .generateScaledMaskForImageForInstances_fromRequestHandler_error(&all, &handler)
                .ok()?;
            CVPixelBufferLockBaseAddress(&pb, CVPixelBufferLockFlags::ReadOnly);
            let (w, h) = (CVPixelBufferGetWidth(&pb), CVPixelBufferGetHeight(&pb));
            let bpr = CVPixelBufferGetBytesPerRow(&pb);
            let base = CVPixelBufferGetBaseAddress(&pb) as *const u8;
            let mut out = Vec::with_capacity(w * h);
            if !base.is_null() {
                for y in 0..h {
                    let row = base.add(y * bpr) as *const f32;
                    for x in 0..w {
                        out.push(*row.add(x));
                    }
                }
            }
            CVPixelBufferUnlockBaseAddress(&pb, CVPixelBufferLockFlags::ReadOnly);
            (out.len() == w * h && w > 0).then_some((out, w as u32, h as u32))
        })
    }
}
