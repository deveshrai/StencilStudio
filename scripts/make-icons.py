#!/usr/bin/env python3
"""Generate all app icons from packaging/icon/hummingbird-split.svg (macOS only:
uses AppKit via scripts/render-svg.swift, sips and iconutil).

Outputs
  packaging/macos/AppIcon.icns                 macOS app icon
  crates/stencil-studio/ui/icons/app-icon.png  window icon
  crates/stencil-studio/android/res/...        Android adaptive + legacy launcher icons
  docs/icon.png                                README logo
"""
import os, re, shutil, subprocess, sys, tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "packaging/icon/hummingbird-split.svg")
RES = os.path.join(ROOT, "crates/stencil-studio/android/res")


def render(svg_text, size, out_png):
    """SVG -> PNG with transparency (AppKit; Quick Look would flatten onto white)."""
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "icon.svg")
        open(p, "w").write(svg_text)
        subprocess.run(["swift", os.path.join(ROOT, "scripts/render-svg.swift"), p, out_png, str(size)],
                       check=True, capture_output=True)


def full_bleed(svg):
    """Android adaptive foreground: background to the edges, bird scaled into the safe zone."""
    s = svg.replace('<g clip-path="url(#tile)">', "<g>")
    s = s.replace('<rect x="100" y="100" width="824" height="824" fill="url(#bg)"/>',
                  '<rect x="0" y="0" width="1024" height="1024" fill="url(#bg)"/>')
    s = s.replace('<rect x="100" y="100" width="824" height="824" fill="url(#bokeh1)"/>',
                  '<rect x="0" y="0" width="1024" height="1024" fill="url(#bokeh1)"/>')
    s = s.replace('<rect x="100" y="100" width="824" height="824" fill="url(#bokeh2)"/>',
                  '<rect x="0" y="0" width="1024" height="1024" fill="url(#bokeh2)"/>')
    s = s.replace('<rect x="512" y="100" width="412" height="824" fill="#151b27"/>',
                  '<rect x="512" y="0" width="512" height="1024" fill="#151b27"/>')
    s = s.replace('<rect x="507" y="100" width="10" height="824" fill="#ffffff"/>',
                  '<rect x="508" y="0" width="8" height="1024" fill="#ffffff"/>')
    s = s.replace('transform="translate(78,18)"',
                  'transform="translate(512 500) scale(0.6) translate(-546 -497) translate(78 18)"')
    # smaller handle, kept inside the safe zone
    s = re.sub(r'<rect x="490" y="690".*?/>\s*<rect x="503".*?/>\s*<rect x="517".*?/>',
               '<rect x="497" y="640" width="30" height="80" rx="15" fill="#ffffff"/>', s, flags=re.S)
    assert "clip-path=\"url(#tile)\"" not in s
    return s


def main():
    svg = open(SRC).read()
    tmp = tempfile.mkdtemp()

    # macOS .icns
    iconset = os.path.join(tmp, "AppIcon.iconset")
    os.makedirs(iconset)
    base = os.path.join(tmp, "base.png")
    render(svg, 1024, base)
    for s in (16, 32, 128, 256, 512):
        subprocess.run(["sips", "-z", str(s), str(s), base, "--out", f"{iconset}/icon_{s}x{s}.png"], check=True, capture_output=True)
        subprocess.run(["sips", "-z", str(s * 2), str(s * 2), base, "--out", f"{iconset}/icon_{s}x{s}@2x.png"], check=True, capture_output=True)
    subprocess.run(["iconutil", "-c", "icns", iconset, "-o", os.path.join(ROOT, "packaging/macos/AppIcon.icns")], check=True)

    # window icon + README logo
    subprocess.run(["sips", "-z", "256", "256", base, "--out", os.path.join(ROOT, "crates/stencil-studio/ui/icons/app-icon.png")], check=True, capture_output=True)
    subprocess.run(["sips", "-z", "256", "256", base, "--out", os.path.join(ROOT, "docs/icon.png")], check=True, capture_output=True)

    # Android: legacy (rounded tile, cropped to the tile) + adaptive foreground
    tile = os.path.join(tmp, "tile.png")
    subprocess.run(["sips", "-c", "824", "824", base, "--out", tile], check=True, capture_output=True)
    fg_svg = full_bleed(svg)
    fg = os.path.join(tmp, "fg.png")
    render(fg_svg, 1024, fg)
    for dpi, legacy, adaptive in (("mdpi", 48, 108), ("hdpi", 72, 162), ("xhdpi", 96, 216),
                                  ("xxhdpi", 144, 324), ("xxxhdpi", 192, 432)):
        d = os.path.join(RES, f"mipmap-{dpi}")
        os.makedirs(d, exist_ok=True)
        subprocess.run(["sips", "-z", str(legacy), str(legacy), tile, "--out", f"{d}/ic_launcher.png"], check=True, capture_output=True)
        subprocess.run(["sips", "-z", str(adaptive), str(adaptive), fg, "--out", f"{d}/ic_launcher_foreground.png"], check=True, capture_output=True)
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
        '    <color name="ic_launcher_background">#151B27</color>\n</resources>\n')
    shutil.copy(fg, os.path.join(tmp, "android-foreground-preview.png"))
    print("icons written; previews in", tmp)


if __name__ == "__main__":
    sys.exit(main())
