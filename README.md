# StencilStudio

Turn any photo into **multi-layer stencils** (hand-cut, laser, flat 3D print) or **paint-by-numbers templates**, matched to the physical paints, pens and pencils you actually own.

Native, keyboard-and-mouse-first "pro" app (ribbon + menus + shortcuts) written in Rust with [Slint](https://slint.dev). Targets macOS, Windows, Linux, Android tablets and iPad.

## Layout

| Crate | Purpose |
|---|---|
| `crates/stencil-core` | UI-free engine: CIELAB/CIEDE2000 color math, palette selection, quantization, region segmentation & small-region merging, preview rendering. Will grow vectorization and PDF/SVG/DXF/STL export. |
| `crates/stencil-studio` | Slint desktop/tablet app. UI lives in `ui/*.slint`; processing runs on a background worker thread (`src/worker.rs`). |

## Run

```sh
cargo run -p stencil-studio                 # app
cargo run -p stencil-studio -- photo.jpg    # open an image directly
cargo test --workspace
cargo run -p stencil-core --release --example reduce -- in.jpg 4 out.png [palette.json]
```

## How the engine works (milestone 1)

1. The image is downscaled to ≤1600 px for live preview and optionally blurred (smoothing).
2. A 5-bit-per-channel histogram is built; buckets are converted to CIELAB.
3. **Palette selection** picks the *k* of the user's colors that minimize the pixel-weighted CIEDE2000 error (greedy + swap refinement), with colorful pixels weighted up so salient subjects keep their hue.
   In *Auto* mode the palette is generated from the image with weighted k-means++.
4. Pixels map to their nearest selected color, are labeled into 4-connected regions, and regions smaller than the **minimum region size (mm²)** at the chosen paper size are merged into the neighbor sharing the longest border.

## Keyboard shortcuts

`Ctrl` = `⌘` on macOS. Full list: **Help → Keyboard Shortcuts** (`Ctrl+/`).

| | |
|---|---|
| Ctrl+O | Open image |
| Ctrl+1 / 2 / 3 / 4 | Original / Preview / Outlines / Split |
| Ctrl+= / Ctrl+- / Ctrl+0 | Zoom in / out / fit (Ctrl+scroll zooms at cursor, drag pans) |
| Ctrl+] / Ctrl+[ | More / fewer colors |
| Ctrl+Alt+S / P | Stencil / Paint-by-Numbers mode |
| Ctrl+F1 | Collapse ribbon |

## Roadmap

- **M1 (done):** engine core, app shell, live preview, palettes (built-in, load/save JSON, edit, auto).
- **M2:** vectorization (potrace-style Béziers); PDF packet (outlines, legend, thumbnail); SVG/DXF stencil layers; registration marks; manual bridges; paint-by-numbers number placement (pole of inaccessibility).
- **M3:** printable palette calibration sheet + photo/scan import (fiducials, white balance, median sampling); color wheel picker; crop/contrast/background removal; project files.
- **Phase 2:** STL/3MF, auto island detection & bridging, kerf compensation, paint-mixing suggestions, Android/iPad packaging.
