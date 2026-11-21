<img src="docs/icon.png" width="112" align="right" alt="StencilStudio icon">

# StencilStudio

Turn any photo or illustration into **multi-layer stencils** (hand-cut, laser-cut or 3D-printed) or **paint-by-numbers templates**, using the colors of the paints, pens and pencils you actually own.

![StencilStudio: a hummingbird photo on the left, its 7-color stencil preview on the right](docs/screenshots/stencil-split.png)

StencilStudio is a native, keyboard-and-mouse-first desktop app with a ribbon, menus and shortcuts. It's written in Rust with [Slint](https://slint.dev). macOS builds are available now; Windows, Linux, Android tablets and iPad are planned.

[![Made with Slint](https://raw.githubusercontent.com/slint-ui/slint/master/logo/MadeWithSlint-logo-whitebg.png)](https://slint.dev)

## Download

macOS pre-release builds (universal: Apple Silicon + Intel, macOS 11+) are on the
[**Releases**](https://github.com/deveshrai/StencilStudio/releases) page.

These builds are ad-hoc signed and **not notarized**. The first time you open the app, right-click it and choose **Open**, or run:

```sh
xattr -dr com.apple.quarantine /Applications/StencilStudio.app
```

### Android (preview)

StencilStudio also runs on Android tablets and foldables (Android 10+, arm64). It has the same ribbon UI with touch on top: two-finger pinch to zoom, drag to pan, and keyboard shortcuts when a keyboard is attached. It has been tested on a Galaxy Z Fold and a Pixel Tablet emulator. Differences from desktop:

- **Open Image** and **Import Scan** offer **Camera…**, the system **Photos** picker (no permission needed), **Browse Files…** (the Files app, including Drive and other providers), and a quick list of recent images in Pictures, DCIM, Download and Documents. Android asks for photo access the first time you use that list.
- **Printing** opens the Android print dialog (any printer, or *Save as PDF*). **Saving** a chart or sheet stores it in **Documents/StencilStudio** and opens it in your PDF viewer. Stencil exports go to the same folder.
- The desktop menu bar is hidden; everything is in the ribbon. On narrow screens, use the ‹ › buttons to scroll the ribbon.

To build: `scripts/build-android.sh --install <adb-serial>`. This needs the Android SDK with **NDK 30+** and **build-tools 37+**, plus `cargo install cargo-apk2` and `rustup target add aarch64-linux-android`. The APK is signed with your local debug key for sideloading.

## How it works

1. **Open an image.** Use JPG, PNG or WebP (`Ctrl+O`, or drop a path on the command line).
2. **Tell it which colors you own.** Pick a built-in palette, load or save your own, or let it derive colors from the image ("Auto").
   For an exact physical match, use the [manual color chart](#calibrating-to-your-real-paints).
3. **Optionally remove the background** (**Remove BG**, `Ctrl+Shift+B`). The subject is cut out and gets all the colours; the background becomes one flat colour of your choice (see [below](#remove-background)).
4. **Choose a mode.** **Stencil** creates one cut sheet per color. **Paint by Numbers** creates an outline template.
5. **Tune it live.** Use these controls:
   - the number of colors;
   - the *minimum region size* in mm², measured at your chosen paper size, so nothing is too small to cut or paint;
   - curve smoothing;
   - pre-smoothing.

   The preview updates within about 0.1 s.
6. **Export.** Choose SVG or DXF for laser/vinyl cutters, or STL for 3D printing (`Ctrl+E`).

Colors are matched perceptually (CIELAB / CIEDE2000), not by raw RGB. When you limit the number of colors, StencilStudio picks the subset of *your* palette that best reproduces the image. It weighs that choice toward what you'd notice:

- **Vivid colors** count more, so a red subject stays red rather than turning into a "safe" brown.
- **Sharp, in-focus areas** count more (*Focus on subject*, on by default under **Image → Color Matching**), so a subject in front of a blurry background gets its colors first. In the hummingbird above, this is what keeps the red throat. Without it, the brown bokeh background takes that color slot.

## Remove background

![The hummingbird with its background removed, previewed on a recommended Sap Green](docs/screenshots/remove-background.png)

**Remove BG** (Home tab, Image menu, or `Ctrl+Shift+B`) separates the subject from its background. Then:

- **Every colour goes to the subject.** In the 3-colour example above, the bird gets yellow, red and blue, instead of spending a colour on the dark bokeh.
- **The background is one flat area.** It never becomes a stencil layer, and it's left as a single region in paint-by-numbers. Paint it in your chosen colour first, or leave the surface bare.
- **Pick the preview background:** **White**, **Black**, one of three **recommended** paints from your palette (chosen to stand out against the subject's main colours), or **Custom** (hue/saturation/lightness sliders or a hex code).

Subject detection uses the best that's available on your device, and the status bar shows which one was used:

- **macOS 14+: Apple Vision.** This is the same subject lifting as Photos and Preview (`VNGenerateForegroundInstanceMaskRequest`).
- **Android, Windows, Linux and older macOS:** a bundled on-device network, [U²-Net-p](https://github.com/xuebinqin/U-2-Net) (4.6 MB, Apache-2.0), run on the CPU with [tract](https://github.com/sonos/tract). It takes about 0.5 s on a laptop and about 1 s on a tablet, once per photo.
- **Fallback:** a colour-model segmentation that needs no model.

Either way, the detector's soft mask is snapped to the photo's real edges at full resolution, and exports use the same detection at export resolution. Nothing leaves the device. If the detector treats something in front of the subject as part of it (a branch held in front of a face, for example), a brush to correct the mask is planned.

## Stencil mode

![Cut / outline view of a portrait stencil](docs/screenshots/stencil-outlines.png)

The **Outlines** view (`Ctrl+3`) shows exactly where the cuts will be. Outlines are true vector curves:

- Each border between two colors is smoothed **once** and shared by both sides, so neighboring shapes always meet exactly. There are no slivers or gaps between layers.
- **Curve smoothing** is set in millimetres at the printed size, so the preview and the exported file match.

### Exported layers

![Four exported stencil layers with bridges and registration holes](docs/screenshots/stencil-layers.png)

*The four layers exported from the hummingbird at 4 colors: orange, blue, sienna, black (gray = sheet material, white = cut away).*

**Export** writes one file per color, lightest first, named `<name>-L<n>-<color>.<ext>`:

| Format | For | Details |
|---|---|---|
| **SVG** | Laser / vinyl cutters | mm units, red 0.01 mm hairline cuts, blue engraved layer label, Inkscape layers *Cut* / *Engrave* |
| **DXF** | Laser, CNC, CAD | AutoCAD R12 (widest compatibility), mm, layers `CUT` and `ENGRAVE` |
| **STL** | 3D printing | Watertight, manifold sheet at your chosen thickness (0.6–3 mm, e.g. 0.8 / 1 / 2 mm) |

Every layer gets a **solid frame** (12 mm by default) and **four round registration holes** in the same place on every sheet. Push pins or dowels through them to line the layers up while you paint.

### Layers view: see and edit bridges

![Layers view on a tablet: the black layer with automatic (blue) bridges and one added by hand (green)](docs/screenshots/layers-bridges.png)

**Layers** (`Ctrl+5`, or *Preview → Layers*) shows each stencil sheet exactly as it will be cut. Pick a layer from the chips at the top.

- **What you see:** cut-outs in the layer's paint color on the sheet material. Automatic bridges are **blue**, bridges you added are **green**, and anything still floating is **red**.
- **Click a bridge** to remove it.
- **Click inside a cut** to add a bridge across it at that spot. It runs horizontally or vertically, whichever is shorter, to the material on both sides. Clicking works with a finger too.
- **Undo** with `Ctrl+Z`. **Reset bridges** returns the layer to automatic.
- **Exports match:** edits are stored in millimetres on the sheet, so SVG, DXF and STL exports contain exactly what you see.

### Floating pieces and bridges

Some cut-outs leave a piece of sheet with nothing holding it, like the center of an "O" or the pupil of an eye. That piece would fall out. StencilStudio decides how to handle each one by its size:

- **Large enough pieces** are held by **two thin bridges on opposite sides**. Each bridge is a straight strip to the nearest solid material; left+right or up+down is chosen, whichever is shorter. In the black layer above (bottom right), the whole bird would otherwise drop out of the sheet; bridges hold it in place.
- **Tiny pieces** (below *Drop pieces under*, 6 mm² by default) are dropped, which simply makes that spot part of the paint area.
- **Mid-size pieces** are also dropped when their bridges would be much longer than the piece itself, instead of being held by long spindly strips. Pieces of 100 mm² or more are always kept.

Bridge width (default 1.2 mm), the drop size and an on/off switch are in **Export → Floating Pieces** and in the Properties panel. All three export formats get the bridges. After each export, the status bar reports how many pieces were bridged and how many were dropped.

## Paint-by-numbers mode

![Paint-by-numbers preview of a portrait with 8 colors](docs/screenshots/paint-by-numbers.png)

Paint-by-numbers uses the same color engine and minimum-region rules, so every cell is big enough to paint. Outlines can be **smooth vector curves** or **pixel-exact**.
*(Number placement and the printable PDF packet with a color legend are next on the roadmap.)*

## Calibrating to your real paints

![Manual color chart, "My Paints" record sheet, and the scan sheet](docs/screenshots/calibration.png)

Paint on paper never looks quite like it does on screen, so StencilStudio offers two printable aids (**Palette** tab, or **File → Calibration**). They print on A4 or US Letter, following your selected paper size.

**Manual Color Chart** (`Ctrl+P`), which you match by eye:

1. Print the chart. Page 1 has 208 color patches with grid codes: 6 vivid rows, 6 muted/earth-tone rows and a gray row. Page 2, *My Paints*, has a box for each of your colors.
2. Dab each paint in its box and let it dry.
3. In daylight, hold each dab next to the chart and find the closest patch.
4. In the app, select that color in the palette, type the patch code (e.g. `D7`) in the **Chart** field, and press Enter.

The palette list shows each color's nearest chart code, so you can also check existing colors at a glance. You match against the *printout*, not the screen, which removes most of the screen-vs-printer mismatch.

**Scan Sheet**, which you fill in, photograph and import:

1. Print it (**Scan Sheet**), fill each box with one paint, and let it dry. Boxes are numbered in the same order as your palette.
2. **Do one eyeball check first:** compare 2–3 paints against the Manual Color Chart in daylight. The app reminds you once per session. Cameras and scanners shift colour with the light, and this one check catches it.
3. **Import Scan** (Palette tab, or File → Calibration). You can use:
   - a photo (JPG, PNG or WebP);
   - a scanner's or scanner app's **PDF**;
   - on Android, the **camera** directly.
4. Review the result. Each box's colour is shown next to the palette colour it will replace; then choose **Apply Colours**.

![Reviewing colours read from a scanned calibration sheet on Android](docs/screenshots/scan-import.png)

How the reader works:

- **Corner markers:** it finds the four markers by shape. The hollow one marks the bottom-right corner, so a rotated or upside-down photo is fine.
- **Perspective and paper:** it corrects perspective and tells A4 from US Letter by itself.
- **Colour:** it uses the printed gray strip to remove colour casts and exposure errors.

In tests with a warm cast and a 90° rotation, colours came back within 1–2 RGB levels. White and very pale paints can't be told apart from the paper, so they're reported as empty; set those by hand. With the *Auto* palette selected, the scanned colours become a new palette, "Scanned paints".

## Keyboard shortcuts

`Ctrl` = `⌘` on macOS. The full list is under **Help → Keyboard Shortcuts** (`Ctrl+/`).

| Keys | Action |
|---|---|
| Ctrl+O | Open image |
| Ctrl+E | Export stencil layers (SVG) |
| Ctrl+P | Print manual color chart |
| Ctrl+1 / 2 / 3 / 4 | Original / Preview / Outlines / Split view |
| Ctrl+5 | Stencil layers (click to add/remove bridges) |
| Ctrl+Z | Undo bridge edit |
| Ctrl+= / Ctrl+- / Ctrl+0 | Zoom in / out / fit (Ctrl+scroll zooms at the cursor; drag to pan) |
| Ctrl+Alt+0 | Actual pixels |
| Ctrl+] / Ctrl+[ | More / fewer colors |
| Ctrl+Shift+B | Remove / restore the background |
| Ctrl+Alt+S / P | Stencil / Paint-by-Numbers mode |
| Ctrl+Alt+L / R | Toggle palette / properties panel |
| Ctrl+F1 | Collapse the ribbon |

## Building from source

Requires a recent stable Rust toolchain.

```sh
cargo run -p stencil-studio                     # run the app
cargo run -p stencil-studio -- photo.jpg        # open an image directly
cargo test --workspace                          # run the tests
scripts/package-macos.sh                        # universal .app + zip in dist/
scripts/build-android.sh --install <serial>     # Android APK in dist/ (+ install)
```

Developer tools:

```sh
# Full pipeline without the UI: previews + per-layer SVG/DXF/STL
cargo run -p stencil-core --release --example reduce -- in.jpg 4 out_dir/ [palette.json]
# Background-removal preview (removed area in magenta); add `model` to skip Apple Vision
cargo run -p stencil-studio --release --example subject -- in.jpg cutout.png [model]
# Read a photographed / scanned / PDF calibration sheet
cargo run -p stencil-core --release --example scan_read -- sheet.jpg
# Re-trace the logo and regenerate every app icon (macOS)
python3 scripts/make-icons.py
# Calibration PDFs
cargo run -p stencil-core --example calibration_sheet -- sheet.pdf [letter]
```

| Crate | Purpose |
|---|---|
| `crates/stencil-core` | UI-free engine: color science, palette selection, segmentation, smooth vectorization, stencil sheets & bridges, SVG/DXF/STL writers, PDF calibration sheets. |
| `crates/stencil-studio` | The Slint app (desktop binary + Android `cdylib`). UI in `ui/*.slint`; platform differences (file dialogs, saving, printing) in `src/platform.rs`; live processing on a background thread; exports run at higher resolution on their own thread. |

<details>
<summary>How the engine works</summary>

1. The image is downscaled to ≤1600 px for the live preview (≤3000 px for export) and optionally pre-blurred.
2. A 5-bit-per-channel histogram is converted to CIELAB.
3. **Palette selection** picks the *k* colors from your palette that minimize the pixel-weighted CIEDE2000 error (greedy + swap refinement), with chroma-weighted saliency. *Auto* mode runs weighted k-means++ instead.
4. With **Remove background**, a subject mask is estimated first (Apple Vision or U²-Net-p, edge-refined with colour models; see above); background pixels are left out of the histogram and become one extra "Background" region that is never a layer.
5. Pixels are mapped to the chosen colors and labeled into 4-connected regions. Regions smaller than the minimum size (mm² at print size) merge into the neighbor they share the longest border with.
6. **Vectorization:** region borders are split into chains between junctions. Each chain is Gaussian-smoothed once with its ends pinned, so every outline built from them shares identical edges.
7. **Stencil sheets:** for each color, solid material = frame + every other color. Floating pieces are bridged or dropped on a pixel mask; the smooth outlines are then merged with the bridges using a robust polygon union (i_overlay), which also removes any self-intersections.
8. **STL:** ear-clipping triangulation with tiny deterministic jitter (removes collinear T-junctions) and pinch-point separation, so the mesh is strictly manifold.

</details>

## Roadmap

- **0.1.0:** engine, app shell, live preview, palettes; smooth vector stencils; SVG/DXF/STL export; scan sheet; macOS build.
- **0.1.1:** bridges for floating pieces (size-aware drop/bridge), manual color chart + chart codes, README walkthrough.
- **0.1.2:** *Focus on subject* color selection; STL robustness fixes found on high-res photos; new screenshots.
- **0.1.4:** Layers view with click-to-add/remove bridges and undo; new puppy app icon.
- **0.1.5:** Remove background (white / black / recommended / custom preview colour); hummingbird app icon traced to clean vectors and a startup animation; Android system Photos and Files pickers.
- **0.1.6:** subject detection via Apple Vision (macOS) and a bundled on-device model (Android and others); calibration scan import from a photo, PDF or the Android camera; Android print dialog, and saved PDFs open in a viewer.
- **Next:** kerf offset, paint-by-numbers numbers + PDF packet, color wheel, crop/contrast, a brush to fix the background mask, project files.
- **Android preview:** same app on tablets/foldables; in-app image browser, pinch-zoom, touch-friendly ribbon.
- **Later:** iPad build (Apple Vision subject lifting, camera, document scanner), Android share intents, 3MF, paint-mixing suggestions, Windows/Linux packages, iPad build.

*Screenshots use royalty-free stock photos, for demonstration only. They are not included in the repository. `docs/sample/balloon.jpg` is an original illustration made for this project that you can use for testing.*
