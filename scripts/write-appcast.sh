#!/usr/bin/env bash
# Writes a one-item Sparkle appcast for a signed update archive.
# Usage: scripts/write-appcast.sh <archive.zip> <version> <download-url> <out.xml> [notes.md] [release-page-url]
# The EdDSA signature comes from sign_update: the login keychain by default, or
# SPARKLE_SIGN_ARGS (e.g. "--ed-key-file key.txt") for a key file.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=scripts/sparkle-tools.sh
source "$ROOT/scripts/sparkle-tools.sh"

ARCHIVE="${1:?usage: write-appcast.sh <archive> <version> <url> <out.xml> [notes.md] [release-page-url]}"
VERSION="${2:?version}"
URL="${3:?download url}"
OUT="${4:?output file}"
NOTES="${5:-}"
PAGE="${6:-}"

read -r -a SIGN_ARGS <<<"${SPARKLE_SIGN_ARGS:-}"
SIG="$("$SPARKLE_BIN/sign_update" ${SIGN_ARGS[@]+"${SIGN_ARGS[@]}"} -p "$ARCHIVE")"
LENGTH="$(wc -c <"$ARCHIVE" | tr -d ' ')"
PUBDATE="$(LC_ALL=C date -u '+%a, %d %b %Y %H:%M:%S +0000')"

xml_escape() { sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g' -e 's/"/\&quot;/g'; }
DESCRIPTION=""
if [[ -n "$NOTES" && -s "$NOTES" ]]; then
  # CDATA can't contain "]]>"; split it if the notes ever do.
  DESCRIPTION="      <description sparkle:format=\"markdown\"><![CDATA[$(sed 's/]]>/]]]]><![CDATA[>/g' "$NOTES")]]></description>"
fi
PAGE_LINK=""
[[ -n "$PAGE" ]] && PAGE_LINK="      <sparkle:fullReleaseNotesLink>$(xml_escape <<<"$PAGE")</sparkle:fullReleaseNotesLink>"

mkdir -p "$(dirname "$OUT")"
cat >"$OUT" <<XML
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>dbear</title>
    <item>
      <title>dbear $VERSION</title>
      <pubDate>$PUBDATE</pubDate>
      <sparkle:version>$VERSION</sparkle:version>
      <sparkle:shortVersionString>$VERSION</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>15.0</sparkle:minimumSystemVersion>
      <sparkle:hardwareRequirements>arm64</sparkle:hardwareRequirements>
${PAGE_LINK}
${DESCRIPTION}
      <enclosure url="$(xml_escape <<<"$URL")" length="$LENGTH" type="application/octet-stream" sparkle:edSignature="$SIG"/>
    </item>
  </channel>
</rss>
XML
xmllint --noout "$OUT"
echo "$OUT"
