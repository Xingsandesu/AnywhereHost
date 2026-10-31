#!/bin/bash
# Prepare a signed update. Pass --publish to push the tag and publish the release.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
if [[ "${1:-}" != "" && "${1:-}" != "--publish" ]]; then
  printf 'Usage: Scripts/release.sh [--publish]\n' >&2; exit 1
fi
if [[ "${1:-}" == "--publish" ]]; then
  [[ -z "$(git status --porcelain)" ]] || { printf 'Commit source changes before publishing.\n' >&2; exit 1; }
  [[ -f "Docs/Releases/$(python3 Scripts/version.py | cut -d ' ' -f 1).md" ]] || { printf 'Missing release notes.\n' >&2; exit 1; }
fi
python3 Scripts/version.py
APP="$ROOT/dist/AnywhereHost.app"
python3 Scripts/verify-app.py "$APP"
VERSION=$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$APP/Contents/Info.plist")
BUILD=$(/usr/libexec/PlistBuddy -c 'Print CFBundleVersion' "$APP/Contents/Info.plist")
[[ "$(python3 Scripts/version.py)" == "$VERSION ($BUILD)" ]] || { printf 'Rebuild the app after changing version.\n' >&2; exit 1; }
REPO=Xingsandesu/AnywhereHost
TAG="v$VERSION"
TOOLS="${SPARKLE_TOOLS:-$ROOT/.build/artifacts/sparkle/Sparkle/bin}"
[[ -x "$TOOLS/generate_appcast" ]] || { printf 'Run swift package resolve first, or set SPARKLE_TOOLS.\n' >&2; exit 1; }
PUBLIC_KEY=$(/usr/libexec/PlistBuddy -c 'Print SUPublicEDKey' "$APP/Contents/Info.plist")
[[ "$PUBLIC_KEY" == "$("$TOOLS/generate_keys" --account com.jintongshu.AnywhereHost -p)" ]] || { printf 'Signing key does not match app.\n' >&2; exit 1; }
OUTPUT="$ROOT/dist/releases/$TAG"
[[ ! -e "$OUTPUT" ]] || { printf 'Release output already exists: %s\n' "$OUTPUT" >&2; exit 1; }
mkdir -p "$OUTPUT"
ARCHIVE="AnywhereHost-$VERSION-arm64.zip"
ditto -c -k --sequesterRsrc --keepParent "$APP" "$OUTPUT/$ARCHIVE"
"$TOOLS/generate_appcast" --account com.jintongshu.AnywhereHost --maximum-deltas 0 \
  --download-url-prefix "https://github.com/$REPO/releases/download/$TAG/" "$OUTPUT"
(cd "$OUTPUT" && shasum -a 256 "$ARCHIVE" appcast.xml > SHA256SUMS)
python3 Scripts/verify-update.py "$OUTPUT/appcast.xml" "$OUTPUT/$ARCHIVE"
if [[ "${1:-}" == "--publish" ]]; then
  [[ -z "$(git status --porcelain)" ]] || { printf 'Commit source changes before publishing.\n' >&2; exit 1; }
  git tag "$TAG"
  git push origin HEAD:main "$TAG"
  gh release create "$TAG" --repo "$REPO" --verify-tag --title "AnywhereHost $VERSION ($BUILD)" \
    --notes-file "Docs/Releases/$VERSION.md" "$OUTPUT/$ARCHIVE" "$OUTPUT/appcast.xml" "$OUTPUT/SHA256SUMS"
fi
printf 'Prepared %s\n' "$OUTPUT"
