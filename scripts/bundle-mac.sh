#!/usr/bin/env bash
# Builds the Rust core + dbear and wraps it in a minimal .app bundle at build/dbear.app
# Usage: [VERSION=x.y.z] scripts/bundle-mac.sh [debug|release]   (Swift config; the Rust core is always built in release)
# Auto-update (Sparkle): SPARKLE_FEED_URL / SPARKLE_PUBLIC_KEY override the feed and EdDSA key,
# BUNDLE_ID the bundle id (scripts/test-update.sh uses a throwaway one), APP the output path.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CONFIG="${1:-debug}"
VERSION="${VERSION:-0.1.0}"
BUNDLE_ID="${BUNDLE_ID:-ar.fausto.dbear}"
# Public half of the Sparkle EdDSA key (`generate_keys -p`). The private half lives only in the
# release Mac's login keychain. While empty, the app's updater stays off.
DBEAR_SPARKLE_PUBLIC_KEY="SZ5yoJuzVi8KJyosiMx/NBxolt+ABNVAoIfw9fbavkU="
SPARKLE_PUBLIC_KEY="${SPARKLE_PUBLIC_KEY-$DBEAR_SPARKLE_PUBLIC_KEY}"
SPARKLE_FEED_URL="${SPARKLE_FEED_URL:-https://github.com/faustofusse/dbear/releases/latest/download/appcast.xml}"
# Dev builds never check on their own (Check for Updates… still works).
AUTO_CHECKS=$([[ "$CONFIG" == release ]] && echo true || echo false)
"$ROOT/scripts/build-core.sh" release
cd "$ROOT/apps/macos"
swift build -c "$CONFIG" --product dbear
BIN_DIR="$(swift build -c "$CONFIG" --show-bin-path)"
cd "$ROOT"
APP="${APP:-build/dbear.app}"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Frameworks"
cp "$BIN_DIR/dbear" "$APP/Contents/MacOS/dbear"
# Sparkle.framework (ditto keeps its symlinks). Headers and modules aren't needed at runtime.
SPARKLE="$APP/Contents/Frameworks/Sparkle.framework"
ditto "$BIN_DIR/Sparkle.framework" "$SPARKLE"
rm -rf "$SPARKLE"/{Headers,PrivateHeaders,Modules} "$SPARKLE"/Versions/B/{Headers,PrivateHeaders,Modules}
otool -l "$APP/Contents/MacOS/dbear" | grep -q '@executable_path/../Frameworks' ||
  install_name_tool -add_rpath @executable_path/../Frameworks "$APP/Contents/MacOS/dbear"
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
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key><string>dbear</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>15.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>SUFeedURL</key><string>$SPARKLE_FEED_URL</string>
  <key>SUPublicEDKey</key><string>$SPARKLE_PUBLIC_KEY</string>
  <key>SUEnableAutomaticChecks</key><$AUTO_CHECKS/>
  <key>SUAutomaticallyUpdate</key><true/>
  <key>SUAllowsAutomaticUpdates</key><true/>
  <key>SUScheduledCheckInterval</key><integer>86400</integer>
</dict></plist>
PLIST
# Sign with a stable identity when there is one, so Keychain "Always Allow" survives rebuilds
# (an ad-hoc signature changes every build and macOS asks again). Override with CODESIGN_IDENTITY.
IDENTITY="${CODESIGN_IDENTITY:-$(security find-identity -p codesigning -v 2>/dev/null | sed -n 's/.*"\(Apple Development:[^"]*\)".*/\1/p' | head -1)}"
# DISTRIBUTE=1 adds the hardened runtime + secure timestamp that notarization requires.
SIGN_OPTS=()
[[ "${DISTRIBUTE:-0}" == 1 ]] && SIGN_OPTS=(--options runtime --timestamp)
sign() {
  local out
  out="$(codesign --force ${SIGN_OPTS[@]+"${SIGN_OPTS[@]}"} --sign "${IDENTITY:--}" "$@" 2>&1)" || { echo "$out" >&2; return 1; }
}
# Inside out, no --deep (Sparkle's documented order for Developer ID + notarization).
sign "$SPARKLE/Versions/B/XPCServices/Installer.xpc"
sign --preserve-metadata=entitlements "$SPARKLE/Versions/B/XPCServices/Downloader.xpc"
sign "$SPARKLE/Versions/B/Autoupdate"
sign "$SPARKLE/Versions/B/Updater.app"
sign "$SPARKLE"
sign "$APP"
echo "signed: ${IDENTITY:-ad-hoc}"
echo "$APP"
