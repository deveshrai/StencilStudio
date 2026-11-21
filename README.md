# StencilStudio

Turn any photo into **multi-layer stencils** (hand-cut, laser, flat 3D print) or **paint-by-numbers templates**, matched to the physical paints, pens and pencils you actually own.

[![Made with Slint](https://raw.githubusercontent.com/slint-ui/slint/master/logo/MadeWithSlint-logo-whitebg.png)](https://slint.dev)

Native, keyboard-and-mouse-first "pro" app (ribbon + menus + shortcuts) written in Rust with [Slint](https://slint.dev). Targets macOS, Windows, Linux, Android tablets and iPad.

## Layout

| Crate | Purpose |
|---|---|
| `crates/stencil-core` | UI-free engine: CIELAB/CIEDE2000 color math, palette selection, quantization, region segmentation & small-region merging, preview rendering. Will grow vectorization and PDF/SVG/DXF/STL export. |
| `crates/stencil-studio` | Slint desktop/tablet app. UI lives in `ui/*.slint`; processing runs on a background worker thread (`src/worker.rs`). |

## Download

macOS pre-release builds (universal: Apple Silicon + Intel) are on the [Releases](https://github.com/deveshrai/StencilStudio/releases) page.
They are ad-hoc signed, not notarized: on first launch right-click the app → **Open**, or run
`xattr -dr com.apple.quarantine StencilStudio.app`.

To build the package yourself: `scripts/package-macos.sh` → `dist/StencilStudio-<version>-macos-universal.zip`.

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

## Stencil export

- Outlines are vectorized once for the whole image: every boundary between two regions is smoothed exactly once (Gaussian, radius set in mm), with junctions pinned, so neighbouring shapes share identical edges with no gaps.
- **Export → SVG / DXF / STL** writes one file per color layer (lightest first): `<name>-L<n>-<color>.<ext>`.
  - Sheet = image + solid frame (default 12 mm) + 4 round registration holes, identical on every layer.
  - **SVG**: mm units, red 0.01 mm hairline cuts, blue engraved layer label (Inkscape layers "Cut"/"Engrave").
  - **DXF**: AutoCAD R12, mm, layers `CUT` (closed polylines) and `ENGRAVE` (text).
  - **STL**: binary, watertight 2-manifold sheet at the chosen thickness (0.6–3 mm).
- Floating islands (e.g. the centre of an "O") are reported after export; automatic bridges are next.
- Paint-by-numbers can use smooth vector or pixel-exact outlines.

## Calibration sheet

**Palette → Print Sheet** (`Ctrl+P`) or **Save PDF**: A4 or US Letter (follows the selected paper), corner fiducials, a gray reference strip and 24 numbered swatch boxes pre-labelled with the current palette's color names.

## Keyboard shortcuts

`Ctrl` = `⌘` on macOS. Full list: **Help → Keyboard Shortcuts** (`Ctrl+/`).

| | |
|---|---|
| Ctrl+O | Open image |
| Ctrl+E | Export stencil layers (SVG) |
| Ctrl+P | Print calibration sheet |
| Ctrl+1 / 2 / 3 / 4 | Original / Preview / Outlines / Split |
| Ctrl+= / Ctrl+- / Ctrl+0 | Zoom in / out / fit (Ctrl+scroll zooms at cursor, drag pans) |
| Ctrl+] / Ctrl+[ | More / fewer colors |
| Ctrl+Alt+S / P | Stencil / Paint-by-Numbers mode |
| Ctrl+F1 | Collapse ribbon |

## Roadmap

- **0.1.0 / M1 (done):** engine core, app shell, live preview, palettes (built-in, load/save JSON, edit, auto).
- **0.1.0 (done):** smooth shared-edge vectorization; SVG/DXF/STL stencil layers with frame, registration holes and thickness; calibration sheet PDF + print; smooth/pixel outlines for paint-by-numbers; macOS universal build.
- **Next:** island bridges (auto + manual); kerf offset; paint-by-numbers number placement + PDF packet; calibration-sheet scan import; color wheel; crop/contrast/background removal; project files.
- **Phase 2:** STL/3MF, auto island detection & bridging, kerf compensation, paint-mixing suggestions, Android/iPad packaging.
