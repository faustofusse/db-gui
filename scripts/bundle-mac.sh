#!/usr/bin/env bash
# Builds the Rust core + dbear and wraps it in a minimal .app bundle at build/dbear.app
# Usage: [VERSION=x.y.z] scripts/bundle-mac.sh [debug|release]   (Swift config; the Rust core is always built in release)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CONFIG="${1:-debug}"
VERSION="${VERSION:-0.1.0}"
"$ROOT/scripts/build-core.sh" release
cd "$ROOT/apps/macos"
swift build -c "$CONFIG" --product dbear
BIN="$(swift build -c "$CONFIG" --show-bin-path)/dbear"
cd "$ROOT"
APP=build/dbear.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/dbear"
# App icon from the Icon Composer file: Assets.car (Liquid Glass icon, macOS 26) + AppIcon.icns fallback.
xcrun actool apps/macos/AppIcon.icon --compile "$APP/Contents/Resources" --platform macosx \
  --minimum-deployment-target 15.0 --app-icon AppIcon \
  --output-partial-info-plist "$ROOT/build/icon-partial.plist" >/dev/null
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>dbear</string>
  <key>CFBundleDisplayName</key><string>dbear</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundleIconName</key><string>AppIcon</string>
  <key>CFBundleIdentifier</key><string>ar.fausto.dbear</string>
  <key>CFBundleExecutable</key><string>dbear</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>15.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
# Sign with a stable identity when there is one, so Keychain "Always Allow" survives rebuilds
# (an ad-hoc signature changes every build and macOS asks again). Override with CODESIGN_IDENTITY.
IDENTITY="${CODESIGN_IDENTITY:-$(security find-identity -p codesigning -v 2>/dev/null | sed -n 's/.*"\(Apple Development:[^"]*\)".*/\1/p' | head -1)}"
# DISTRIBUTE=1 adds the hardened runtime + secure timestamp that notarization requires.
SIGN_OPTS=()
[[ "${DISTRIBUTE:-0}" == 1 ]] && SIGN_OPTS=(--options runtime --timestamp)
codesign --force ${SIGN_OPTS[@]+"${SIGN_OPTS[@]}"} --sign "${IDENTITY:--}" "$APP" >/dev/null
echo "signed: ${IDENTITY:-ad-hoc}"
echo "$APP"
