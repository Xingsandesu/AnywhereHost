#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
UNSIGNED=false
if [[ "${1:-}" == "--unsigned" && "$#" == 1 ]]; then
  UNSIGNED=true
elif [[ "$#" != 0 ]]; then
  printf 'Usage: Scripts/build-app.sh [--unsigned]\n' >&2
  exit 1
fi
# Hardened Runtime library validation requires a real common Team ID for Sparkle.
if [[ "$UNSIGNED" == false && -z "${ANYWHERE_SIGNING_IDENTITY:-}" ]]; then
  ANYWHERE_SIGNING_IDENTITY="$(security find-identity -v -p codesigning | python3 -c 'import re,sys; rows=re.findall(r"([A-F0-9]{40}) \"([^\"]+)\"",sys.stdin.read()); print(next((h for prefix in ["Developer ID Application:", "Apple Development:"] for h,n in rows if n.startswith(prefix)), ""))')"
fi
if [[ "$UNSIGNED" == false && ( -z "${ANYWHERE_SIGNING_IDENTITY:-}" || "${ANYWHERE_SIGNING_IDENTITY:-}" == "-" ) ]]; then
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
if [[ "$UNSIGNED" == true ]]; then
  python3 Scripts/verify-app.py "$APP" --unsigned
  printf 'Built unsigned CI bundle %s (not signed for distribution or OTA)\n' "$APP"
  exit 0
fi
ANYWHERE_SIGNING_IDENTITY="$ANYWHERE_SIGNING_IDENTITY" bash "$ROOT/Scripts/sign-app.sh" "$APP"
printf 'Built %s\n' "$APP"
