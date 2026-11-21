# Bundled models

`u2netp.onnx`: U²-Net-p salient object detection (Xuebin Qin, Zichen Zhang, Chenyang Huang,
Masood Dehghan, Osmar R. Zaiane and Martin Jagersand, "U²-Net: Going Deeper with Nested
U-Structure for Salient Object Detection", Pattern Recognition 2020).
<https://github.com/xuebinqin/U-2-Net>, Apache License 2.0.

The ONNX export is the one distributed by rembg (<https://github.com/danielgatis/rembg>),
with one change so the pure-Rust `tract` runtime can load it: the `Resize` nodes'
`coordinate_transformation_mode` was changed from `pytorch_half_pixel` to the
equivalent `half_pixel` (they differ only for 1-pixel outputs, which this network
never produces).

Used for "Remove background" where Apple Vision is not available (Android, Windows,
Linux, macOS before 14).
