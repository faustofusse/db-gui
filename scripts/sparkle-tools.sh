# shellcheck shell=bash
# Sourced by the release/update scripts: sets SPARKLE_BIN to Sparkle's command-line tools
# (generate_keys, sign_update) from the SwiftPM artifact, so their version follows Package.resolved.
SPARKLE_BIN="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/apps/macos/.build/artifacts/sparkle/Sparkle/bin"
if [[ ! -x "$SPARKLE_BIN/sign_update" ]]; then
  (cd "$(dirname "${BASH_SOURCE[0]}")/../apps/macos" && swift package resolve >/dev/null)
fi
[[ -x "$SPARKLE_BIN/sign_update" ]] || { echo "Sparkle tools not found in $SPARKLE_BIN" >&2; exit 1; }
