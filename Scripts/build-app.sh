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
BUILD_OPTIONS=(-project "$ROOT/AnywhereHost.xcodeproj" -scheme AnywhereHost -configuration Release
  -destination 'generic/platform=macOS' -derivedDataPath "${ANYWHERE_DERIVED_DATA:-$ROOT/.build/xcode}"
  CODE_SIGNING_ALLOWED=NO)
xcodebuild "${BUILD_OPTIONS[@]}" clean build
BUILT_APP="$(xcodebuild "${BUILD_OPTIONS[@]}" -showBuildSettings -json | python3 -c '
import json, pathlib, sys
settings = next(row["buildSettings"] for row in json.load(sys.stdin) if row["target"] == "AnywhereHost")
print(pathlib.Path(settings["TARGET_BUILD_DIR"]) / settings["WRAPPER_NAME"])
')"
OUTPUT_DIR="${ANYWHERE_OUTPUT_DIR:-$ROOT/dist}"
APP="$OUTPUT_DIR/AnywhereHost.app"
mkdir -p "$OUTPUT_DIR"
# Assemble a fresh output bundle.
rm -rf "$APP"
ditto "$BUILT_APP" "$APP"
if [[ "$UNSIGNED" == true ]]; then
  python3 Scripts/verify-app.py "$APP" --unsigned
  printf 'Built unsigned CI bundle %s (not signed for distribution or OTA)\n' "$APP"
  exit 0
fi
ANYWHERE_SIGNING_IDENTITY="$ANYWHERE_SIGNING_IDENTITY" bash "$ROOT/Scripts/sign-app.sh" "$APP"
printf 'Built %s\n' "$APP"
