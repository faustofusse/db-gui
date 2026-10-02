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

VERSION="$VERSION" "$ROOT/scripts/bundle-mac.sh" release

ZIP="build/dbear-$VERSION-macos-arm64.zip"
rm -f "$ZIP"
ditto -c -k --sequesterRsrc --keepParent build/dbear.app "$ZIP"
shasum -a 256 "$ZIP" | tee "$ZIP.sha256"

[[ "$PUBLISH" == --publish ]] || { echo "built $ZIP (pass --publish to upload)"; exit 0; }

git tag -a "$TAG" -m "dbear $VERSION"
git push origin "$TAG"
gh release create "$TAG" "$ZIP" "$ZIP.sha256" --title "dbear $VERSION" --generate-notes --notes "$(cat <<'EOF'
Apple silicon, macOS 15 or later.

1. Download the zip, unzip it and move `dbear.app` to `/Applications`.
2. The app is not notarized, so macOS blocks it the first time. Either right-click › Open, allow it in
   System Settings › Privacy & Security › "Open Anyway", or run:
   `xattr -dr com.apple.quarantine /Applications/dbear.app`
EOF
)"
