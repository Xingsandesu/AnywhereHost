#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
# Hardened Runtime library validation requires a real common Team ID for Sparkle.
if [[ -z "${ANYWHERE_SIGNING_IDENTITY:-}" ]]; then
  ANYWHERE_SIGNING_IDENTITY="$(security find-identity -v -p codesigning | python3 -c 'import re,sys; rows=re.findall(r"([A-F0-9]{40}) \"([^\"]+)\"",sys.stdin.read()); print(next((h for prefix in ["Developer ID Application:", "Apple Development:"] for h,n in rows if n.startswith(prefix)), ""))')"
fi
if [[ -z "$ANYWHERE_SIGNING_IDENTITY" || "$ANYWHERE_SIGNING_IDENTITY" == "-" ]]; then
  printf 'A Developer ID Application or Apple Development identity is required for Sparkle with Hardened Runtime. Set ANYWHERE_SIGNING_IDENTITY.\n' >&2
  exit 1
fi
xcodebuild -project AnywhereHost.xcodeproj -scheme AnywhereHost -configuration Release \
  -destination 'generic/platform=macOS' -derivedDataPath "$ROOT/.build/xcode" \
  CODE_SIGNING_ALLOWED=NO clean build
APP="$ROOT/dist/AnywhereHost.app"
mkdir -p "$ROOT/dist"
# Assemble a fresh output bundle.
rm -rf "$APP"
ditto "$ROOT/.build/xcode/Build/Products/Release/AnywhereHost.app" "$APP"
# Sparkle contains nested code. Sign from the inside out, preserving its XPC
# entitlements. The distributed framework's original team cannot load into an
# ad-hoc Hardened Runtime app, so use the same identity for all bundled code.
FRAMEWORK="$APP/Contents/Frameworks/Sparkle.framework"
if [[ -d "$FRAMEWORK" ]]; then
  while IFS= read -r -d '' component; do
    codesign --force --options runtime --preserve-metadata=entitlements --sign "${ANYWHERE_SIGNING_IDENTITY}" "$component"
  done < <(find "$FRAMEWORK/Versions/B" -depth \( -name '*.xpc' -o -name '*.app' -o -name Autoupdate \) -print0)
  codesign --force --options runtime --sign "${ANYWHERE_SIGNING_IDENTITY}" "$FRAMEWORK"
fi
for binary in anywhere-host anywhere-permission-probe frpc; do
  codesign --force --options runtime --sign "${ANYWHERE_SIGNING_IDENTITY}" "$APP/Contents/MacOS/$binary"
done
codesign --force --options runtime --entitlements "$ROOT/Resources/AnywhereHost.entitlements" --sign "${ANYWHERE_SIGNING_IDENTITY}" "$APP"
python3 Scripts/verify-app.py "$APP"
printf 'Built %s\n' "$APP"
