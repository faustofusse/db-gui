#!/usr/bin/env bash
# Builds the Rust core + DBGuiMac and wraps it in a minimal .app bundle at build/DBGui.app
# Usage: scripts/bundle-mac.sh [debug|release]   (Swift config; the Rust core is always built in release)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CONFIG="${1:-debug}"
"$ROOT/scripts/build-core.sh" release
cd "$ROOT/apps/macos"
swift build -c "$CONFIG" --product DBGuiMac
BIN="$(swift build -c "$CONFIG" --show-bin-path)/DBGuiMac"
cd "$ROOT"
APP=build/DBGui.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp "$BIN" "$APP/Contents/MacOS/DBGui"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>DBGui</string>
  <key>CFBundleIdentifier</key><string>dev.fausto.dbgui</string>
  <key>CFBundleExecutable</key><string>DBGui</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>LSMinimumSystemVersion</key><string>15.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
codesign --force --sign - "$APP" >/dev/null
echo "$APP"
