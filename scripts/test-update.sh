#!/usr/bin/env bash
# End-to-end test of Sparkle auto-update against a local feed. Touches nothing real: a throwaway
# EdDSA key, bundle id ar.fausto.dbear.updatetest, a loopback feed, a scratch connections file.
#
# 1. Builds dbear 9.0.0 (the update) and 8.9.0 (installed), both trusting the test key.
# 2. Serves the 9.0.0 zip and its appcast on 127.0.0.1:$PORT.
# 3. Launches 8.9.0 in the background with DBEAR_UPDATE_TEST=1 (check right away); Sparkle must
#    download and stage the update without showing any window, then install it on quit.
# 4. Negative cases: a tampered zip and an archive signed with another key must not install.
#
# Usage: scripts/test-update.sh [--keep]   (PORT=8743 by default; --keep leaves build/update-test)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
PORT="${PORT:-8743}"
BUNDLE_ID=ar.fausto.dbear.updatetest
DIR="$ROOT/build/update-test"
FEED="http://127.0.0.1:$PORT/appcast.xml"
TIMEOUT="${TIMEOUT:-60}"

fail() { echo "FAIL: $*" >&2; exit 1; }
lsof -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1 && fail "port $PORT is busy (set PORT)"
rm -rf "$DIR" && mkdir -p "$DIR/www"

# Throwaway Ed25519 keys in Sparkle's format (base64 raw seed / raw public key).
keygen() {
  cat >"$DIR/keygen.swift" <<'SWIFT'
import CryptoKit
let key = Curve25519.Signing.PrivateKey()
print(key.rawRepresentation.base64EncodedString())
print(key.publicKey.rawRepresentation.base64EncodedString())
SWIFT
  /usr/bin/swift "$DIR/keygen.swift"
}
{ read -r PRIV; read -r PUB; } < <(keygen)
{ read -r OTHER_PRIV; read -r _; } < <(keygen)
echo "$PRIV" >"$DIR/test.key"
echo "$OTHER_PRIV" >"$DIR/other.key"

build() { # <version> <app path>
  local out
  out="$(VERSION="$1" APP="$2" BUNDLE_ID="$BUNDLE_ID" SPARKLE_PUBLIC_KEY="$PUB" SPARKLE_FEED_URL="$FEED" \
    "$ROOT/scripts/bundle-mac.sh" release 2>&1)" || { echo "$out" >&2; fail "building $1"; }
}
echo "== building 9.0.0 and 8.9.0"
build 9.0.0 "$DIR/new/dbear.app"
ZIP="$DIR/www/dbear-9.0.0.zip"
ditto -c -k --sequesterRsrc --keepParent "$DIR/new/dbear.app" "$ZIP"
cp "$ZIP" "$DIR/good.zip"
build 8.9.0 "$DIR/old/dbear.app"

appcast() { # <key file>
  SPARKLE_SIGN_ARGS="--ed-key-file $1" "$ROOT/scripts/write-appcast.sh" "$ZIP" 9.0.0 \
    "http://127.0.0.1:$PORT/dbear-9.0.0.zip" "$DIR/www/appcast.xml" "" >/dev/null
}

python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$DIR/www" >"$DIR/http.log" 2>&1 &
SERVER=$!
cleanup() {
  kill "$SERVER" 2>/dev/null || true
  osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || true
  defaults delete "$BUNDLE_ID" >/dev/null 2>&1 || true
  [[ "$KEEP" == 1 ]] || rm -rf "$DIR"
}
KEEP=0
if [[ "${1:-}" == --keep ]]; then KEEP=1; fi
trap cleanup EXIT

version_of() { /usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$1/Contents/Info.plist"; }

# Window titles the app process has on screen (read-only; Sparkle's UI runs in-process).
windows_of() { # <pid>
  cat >"$DIR/windows.swift" <<'SWIFT'
import CoreGraphics
import Foundation
let pid = Int32(CommandLine.arguments[1])!
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
for w in list where (w[kCGWindowOwnerPID as String] as? Int32) == pid && (w[kCGWindowLayer as String] as? Int) == 0 {
  print("\(w[kCGWindowNumber as String] ?? 0)\t\(w[kCGWindowName as String] as? String ?? "")")
}
SWIFT
  /usr/bin/swift "$DIR/windows.swift" "$1"
}

# Runs the installed app against the current appcast. Echoes the version on disk after quitting.
run_case() { # <name> <expect: ready|none>
  local name="$1" expect="$2" app="$DIR/run/dbear.app" log="$DIR/$1.log"
  rm -rf "$DIR/run" && mkdir -p "$DIR/run" && ditto "$DIR/old/dbear.app" "$app"
  defaults delete "$BUNDLE_ID" >/dev/null 2>&1 || true
  open -g -n --stdout "$log" --stderr "$log" \
    --env DBEAR_UPDATE_TEST=1 --env DBEAR_CONNECTIONS_FILE="$DIR/connections.db" "$app"
  local waited=0
  while ! grep -q "dbear-update-test: cycle finished\|dbear-update-test: ready\|dbear-update-test: aborted" "$log" 2>/dev/null; do
    sleep 1; waited=$((waited + 1)); [[ $waited -lt $TIMEOUT ]] || fail "$name: no update cycle after ${TIMEOUT}s"
  done
  sleep 3
  local pid; pid="$(pgrep -f "$app/Contents/MacOS/dbear" | head -1)"
  local wins; wins="$(windows_of "$pid")"
  echo "   windows: $(cut -f2 <<<"$wins" | tr '\n' '|')"
  # Main window only: the scheduled check must not have opened anything of Sparkle's.
  [[ "$(grep -c . <<<"$wins")" -le 1 ]] || fail "$name: unexpected extra windows"
  if [[ "$name" == good ]]; then
    screencapture -x -o -l "$(head -1 <<<"$wins" | cut -f1)" "$DIR/../update-test-window.png" || true
  fi
  if [[ "$expect" == ready ]]; then
    grep -q "dbear-update-test: ready 9.0.0" "$log" || { cat "$log"; fail "$name: update not staged"; }
    grep -q "dbear-update-test: app menu: .*Check for Updates….*Restart to Update to 9.0.0" "$log" ||
      { cat "$log"; fail "$name: no \"Restart to Update\" in the app menu"; }
  else
    grep -q "dbear-update-test: ready" "$log" && fail "$name: a bad update was staged"
  fi
  osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null
  while pgrep -f "$app/Contents/MacOS/dbear" >/dev/null; do sleep 1; done
  # Sparkle's installer replaces the bundle after the app exits.
  waited=0
  while [[ "$(version_of "$app")" != 9.0.0 && $waited -lt 30 ]]; do sleep 1; waited=$((waited + 1)); done
  [[ "$expect" == ready ]] || sleep 5
  version_of "$app"
}

echo "== good update"
appcast "$DIR/test.key"
v="$(run_case good ready | tee /dev/stderr | tail -1)"
[[ "$v" == 9.0.0 ]] || fail "good: still $v after quitting"
codesign --verify --deep --strict "$DIR/run/dbear.app" || fail "good: updated app fails codesign"
echo "   ok: 8.9.0 → 9.0.0, silently"

echo "== tampered archive"
appcast "$DIR/test.key"
printf 'x' >>"$ZIP"
v="$(run_case tampered none | tee /dev/stderr | tail -1)"
[[ "$v" == 8.9.0 ]] || fail "tampered: installed $v"
cp "$DIR/good.zip" "$ZIP"
echo "   ok: rejected"

echo "== signed with another key"
appcast "$DIR/other.key"
v="$(run_case wrong-key none | tee /dev/stderr | tail -1)"
[[ "$v" == 8.9.0 ]] || fail "wrong key: installed $v"
echo "   ok: rejected"

echo "== settings window (screenshot only)"
rm -f "$DIR/www/appcast.xml"  # no feed: the check fails quietly in the background
log="$DIR/settings.log"
open -g -n --stdout "$log" --stderr "$log" --env DBEAR_UPDATE_TEST=1 --env DBEAR_UPDATE_TEST_SETTINGS=1 \
  --env DBEAR_CONNECTIONS_FILE="$DIR/connections.db" "$DIR/new/dbear.app"
sleep 4
pid="$(pgrep -f "$DIR/new/dbear.app/Contents/MacOS/dbear" | head -1)"
wid="$(windows_of "$pid" | { grep -v $'\tTables$' || true; } | head -1 | cut -f1)"
[[ -n "$wid" ]] && screencapture -x -o -l "$wid" "$ROOT/build/update-test-settings.png" || echo "   (no settings window found)"
osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null

echo "PASS (screenshots: build/update-test-window.png, build/update-test-settings.png)"
