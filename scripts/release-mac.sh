#!/usr/bin/env bash
# Builds a release dbear.app, zips it and (with --publish) uploads it as a GitHub release.
# Usage: scripts/release-mac.sh <version> [--publish]     e.g. scripts/release-mac.sh 0.1.0 --publish
# The tag is v<version> and points at HEAD, so the working tree must be clean to publish.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION="${1:?usage: scripts/release-mac.sh <version> [--publish]}"
VERSION="${VERSION#v}"
PUBLISH="${2:-}"
TAG="v$VERSION"

if [[ "$PUBLISH" == --publish ]]; then
  [[ -z "$(git status --porcelain)" ]] || { echo "working tree is dirty; commit first" >&2; exit 1; }
  git rev-parse -q --verify "refs/tags/$TAG" >/dev/null && { echo "tag $TAG already exists" >&2; exit 1; }
fi

# Signed with Developer ID and notarized with the notarytool keychain profile NOTARY_PROFILE (default
# "dbear"; create it once with `xcrun notarytool store-credentials dbear --apple-id ... --team-id ...`).
NOTARY_PROFILE="${NOTARY_PROFILE:-dbear}"
IDENTITY="${CODESIGN_IDENTITY:-$(security find-identity -p codesigning -v | sed -n 's/.*"\(Developer ID Application:[^"]*\)".*/\1/p' | head -1)}"
[[ -n "$IDENTITY" ]] || { echo "no Developer ID Application certificate in the keychain" >&2; exit 1; }
xcrun notarytool history --keychain-profile "$NOTARY_PROFILE" >/dev/null || {
  echo "no notarytool profile '$NOTARY_PROFILE'; run: xcrun notarytool store-credentials $NOTARY_PROFILE" >&2; exit 1; }

VERSION="$VERSION" CODESIGN_IDENTITY="$IDENTITY" DISTRIBUTE=1 "$ROOT/scripts/bundle-mac.sh" release

ZIP="build/dbear-$VERSION-macos-arm64.zip"
rm -f "$ZIP"
ditto -c -k --sequesterRsrc --keepParent build/dbear.app "$ZIP"
xcrun notarytool submit "$ZIP" --keychain-profile "$NOTARY_PROFILE" --wait
xcrun stapler staple build/dbear.app
spctl --assess --type execute -vv build/dbear.app
# Re-zip so the download carries the stapled ticket (works offline on first launch).
rm -f "$ZIP"
ditto -c -k --sequesterRsrc --keepParent build/dbear.app "$ZIP"
shasum -a 256 "$ZIP" | tee "$ZIP.sha256"

[[ "$PUBLISH" == --publish ]] || { echo "built $ZIP (pass --publish to upload)"; exit 0; }

git tag -a "$TAG" -m "dbear $VERSION"
git push origin "$TAG"
gh release create "$TAG" "$ZIP" "$ZIP.sha256" --title "dbear $VERSION" --generate-notes --notes "$(cat <<'EOF'
Apple silicon, macOS 15 or later.

Download the zip, unzip it and move `dbear.app` to `/Applications`.
EOF
)"
