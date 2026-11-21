#!/usr/bin/env python3
"""Generate all app icons from packaging/icon/hummingbird.svg (macOS only: uses AppKit
via scripts/render-svg.swift, sips and iconutil).

hummingbird.svg holds only the coloured pieces of the logo on a 1024 canvas; it was
traced from the supplied artwork with
  cargo run -p stencil-core --release --example icon_trace -- logo.jpg hummingbird.svg \
      1F1F1F EDEDE7 F5C515 E8721E BA251D 2960AE 854928

Outputs
  packaging/macos/AppIcon.icns                 macOS app icon (rounded dark tile)
  crates/stencil-studio/ui/icons/app-icon.png  window icon
  crates/stencil-studio/ui/splash-logo.png     splash still / fallback
  crates/stencil-studio/android/res/...        Android adaptive + legacy launcher icons
  docs/icon.png                                README logo
"""
import os, re, subprocess, sys, tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "packaging/icon/hummingbird.svg")
RES = os.path.join(ROOT, "crates/stencil-studio/android/res")
BG = "#1F1F1F"
# centre of the bird's bounding box on the 1024 canvas (from icon_trace)
CX, CY = 518.8, 511.4


def run(*a):
    subprocess.run(list(a), check=True, capture_output=True)


def render(svg_text, size, out_png):
    """SVG -> PNG with transparency (AppKit; Quick Look would flatten onto white)."""
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "icon.svg")
        open(p, "w").write(svg_text)
        run("swift", os.path.join(ROOT, "scripts/render-svg.swift"), p, out_png, str(size))


def pieces():
    s = open(SRC).read()
    return "".join(re.findall(r"<path [^>]*/>", s))


def bird(scale, dx=0.0, dy=0.0):
    return (f'<g transform="translate({512 + dx} {512 + dy}) scale({scale}) translate({-CX} {-CY})">'
            f"{pieces()}</g>")


def tile_svg():
    """macOS-style rounded tile: soft dark gradient, bird slightly lifted with a shadow."""
    return f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">
<defs>
  <linearGradient id="g" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0" stop-color="#2B2B2B"/><stop offset="1" stop-color="#171717"/>
  </linearGradient>
  <filter id="s" x="-10%" y="-10%" width="120%" height="120%">
    <feDropShadow dx="0" dy="6" stdDeviation="7" flood-color="#000" flood-opacity="0.55"/>
  </filter>
</defs>
<rect x="100" y="100" width="824" height="824" rx="185" fill="url(#g)"/>
<rect x="101.5" y="101.5" width="821" height="821" rx="184" fill="none" stroke="#ffffff" stroke-opacity="0.07" stroke-width="3"/>
<g filter="url(#s)">{bird(0.93, -6, 4)}</g>
</svg>'''


def foreground_svg():
    """Android adaptive foreground: transparent, bird inside the central safe zone."""
    return f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">
<defs><filter id="s" x="-10%" y="-10%" width="120%" height="120%">
  <feDropShadow dx="0" dy="5" stdDeviation="6" flood-color="#000" flood-opacity="0.5"/></filter></defs>
<g filter="url(#s)">{bird(0.74, -3, 4)}</g>
</svg>'''


def splash_svg():
    return f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">{bird(1.0, -6.8, 0.6)}</svg>'''


def main():
    tmp = tempfile.mkdtemp()

    base = os.path.join(tmp, "base.png")
    render(tile_svg(), 1024, base)
    iconset = os.path.join(tmp, "AppIcon.iconset")
    os.makedirs(iconset)
    for s in (16, 32, 128, 256, 512):
        run("sips", "-z", str(s), str(s), base, "--out", f"{iconset}/icon_{s}x{s}.png")
        run("sips", "-z", str(s * 2), str(s * 2), base, "--out", f"{iconset}/icon_{s}x{s}@2x.png")
    subprocess.run(["iconutil", "-c", "icns", iconset, "-o", os.path.join(ROOT, "packaging/macos/AppIcon.icns")], check=True)

    run("sips", "-z", "256", "256", base, "--out", os.path.join(ROOT, "crates/stencil-studio/ui/icons/app-icon.png"))
    run("sips", "-z", "256", "256", base, "--out", os.path.join(ROOT, "docs/icon.png"))
    render(splash_svg(), 512, os.path.join(ROOT, "crates/stencil-studio/ui/splash-logo.png"))

    # Android: legacy (the rounded tile) + adaptive foreground on a flat backdrop
    tile = os.path.join(tmp, "tile.png")
    run("sips", "-c", "824", "824", base, "--out", tile)
    fg = os.path.join(tmp, "fg.png")
    render(foreground_svg(), 1024, fg)
    for dpi, legacy, adaptive in (("mdpi", 48, 108), ("hdpi", 72, 162), ("xhdpi", 96, 216),
                                  ("xxhdpi", 144, 324), ("xxxhdpi", 192, 432)):
        d = os.path.join(RES, f"mipmap-{dpi}")
        os.makedirs(d, exist_ok=True)
        run("sips", "-z", str(legacy), str(legacy), tile, "--out", f"{d}/ic_launcher.png")
        run("sips", "-z", str(adaptive), str(adaptive), fg, "--out", f"{d}/ic_launcher_foreground.png")
    any_dir = os.path.join(RES, "mipmap-anydpi-v26")
    os.makedirs(any_dir, exist_ok=True)
    open(os.path.join(any_dir, "ic_launcher.xml"), "w").write(
        '<?xml version="1.0" encoding="utf-8"?>\n'
        '<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">\n'
        '    <background android:drawable="@color/ic_launcher_background"/>\n'
        '    <foreground android:drawable="@mipmap/ic_launcher_foreground"/>\n'
        '</adaptive-icon>\n')
    open(os.path.join(RES, "values/colors.xml"), "w").write(
        '<?xml version="1.0" encoding="utf-8"?>\n<resources>\n'
        f'    <color name="ic_launcher_background">{BG}</color>\n</resources>\n')
    print(tmp)


if __name__ == "__main__":
    sys.exit(main())
