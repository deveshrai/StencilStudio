#!/usr/bin/env bash
# Build an optimized StencilStudio APK (arm64; pass --all-abis to add x86_64).
# Signed with the StencilStudio release key if present, else the local debug key.
# Usage: scripts/build-android.sh [--install <adb-serial>] [--all-abis]
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/android-env.sh

TARGETS=(--target aarch64-linux-android)
SERIAL=""
while [ $# -gt 0 ]; do
  case "$1" in
    --all-abis) TARGETS=() ;; # every ABI in [package.metadata.android] build_targets
    --install) SERIAL="$2"; shift ;;
  esac
  shift
done

# Public builds are signed with the StencilStudio release key (kept outside the repo,
# see ~/.config/stencilstudio/keystore.env). Without it, fall back to the debug key.
if [ -f "$HOME/.config/stencilstudio/keystore.env" ]; then
  source "$HOME/.config/stencilstudio/keystore.env"
fi
export CARGO_APK_RELEASE_KEYSTORE="${CARGO_APK_RELEASE_KEYSTORE:-$HOME/.android/debug.keystore}"
export CARGO_APK_RELEASE_KEYSTORE_PASSWORD="${CARGO_APK_RELEASE_KEYSTORE_PASSWORD:-android}"

cargo apk2 build --release --lib -p stencil-studio ${TARGETS[@]+"${TARGETS[@]}"}
APK=target/release/apk/StencilStudio.apk
VERSION=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print([p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="stencil-studio"][0])')
mkdir -p dist
cp "$APK" "dist/StencilStudio-$VERSION-android.apk"
echo "Created dist/StencilStudio-$VERSION-android.apk"

if [ -n "$SERIAL" ]; then
  adb -s "$SERIAL" install -r "$APK"
  adb -s "$SERIAL" shell am start -n io.github.deveshrai.stencilstudio/android.app.NativeActivity
fi
