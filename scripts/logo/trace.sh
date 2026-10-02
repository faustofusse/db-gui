#!/usr/bin/env bash
# Vectorizes assets/logo.jpeg into the app's SVGs:
#   assets/logo.svg                        bear only, transparent
#   assets/icon.svg                        classic macOS icon (squircle + bear)
#   apps/macos/AppIcon.icon/Assets/bear.svg  bear layer for the Icon Composer icon (macOS 26)
# Steps: flatten the JPEG to its three flat colors (quantize.py), trace with vtracer,
# then snap/crop/compose (build_svgs.py). Tools come from nixpkgs.
set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT=$PWD
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
PY='(builtins.getFlake "nixpkgs").legacyPackages.${builtins.currentSystem}.python3.withPackages (p: [p.pillow p.numpy])'

sips -Z 1024 assets/logo.jpeg -s format png --out "$WORK/in.png" >/dev/null
cd "$WORK"
nix shell --impure --expr "$PY" -c python3 "$ROOT/scripts/logo/quantize.py"
nix run nixpkgs#vtracer -- --input flat.png --output traced.svg --colormode color --hierarchical stacked \
  --mode spline --filter_speckle 12 --color_precision 8 --gradient_step 16 --corner_threshold 60 \
  --segment_length 4 --splice_threshold 45 --path_precision 1 >/dev/null
nix shell --impure --expr "$PY" -c python3 "$ROOT/scripts/logo/build_svgs.py"
cp dbear-logo.svg "$ROOT/assets/logo.svg"
cp dbear-icon.svg "$ROOT/assets/icon.svg"
cp bear-layer.svg "$ROOT/apps/macos/AppIcon.icon/Assets/bear.svg"
echo "updated assets/logo.svg, assets/icon.svg, apps/macos/AppIcon.icon/Assets/bear.svg"
