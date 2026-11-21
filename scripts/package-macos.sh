#!/usr/bin/env bash
# Build a universal (Apple Silicon + Intel) StencilStudio.app and zip it into dist/.
# Usage: scripts/package-macos.sh
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print([p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="stencil-studio"][0])')
APP="dist/StencilStudio.app"
echo "Packaging StencilStudio $VERSION"

for t in aarch64-apple-darwin x86_64-apple-darwin; do
  MACOSX_DEPLOYMENT_TARGET=11.0 cargo build --release -p stencil-studio --target "$t"
done

rm -rf dist && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/MacOS/StencilStudio" \
  target/aarch64-apple-darwin/release/stencil-studio \
  target/x86_64-apple-darwin/release/stencil-studio

# Icon (generated from packaging/icon/hummingbird-split.svg by scripts/make-icons.py)
cp packaging/macos/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"

sed "s/__VERSION__/$VERSION/g" packaging/macos/Info.plist > "$APP/Contents/Info.plist"

# Ad-hoc signature (required to run on Apple Silicon). Not notarized.
codesign --force --deep --sign - "$APP"

ZIP="dist/StencilStudio-$VERSION-macos-universal.zip"
ditto -c -k --keepParent "$APP" "$ZIP"
echo "Created $ZIP"
