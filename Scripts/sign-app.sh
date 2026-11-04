#!/bin/bash
# Sign an assembled app, including Sparkle's nested code, from the inside out.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[[ "$#" == 1 && -d "$1/Contents" ]] || { printf 'Usage: Scripts/sign-app.sh APP\n' >&2; exit 1; }
: "${ANYWHERE_SIGNING_IDENTITY:?Set a real signing identity}"
[[ "$ANYWHERE_SIGNING_IDENTITY" != - ]] || { printf 'Ad-hoc signing is not supported.\n' >&2; exit 1; }
APP="$1"
: "${ANYWHERE_PROVISIONING_PROFILE:?Set the Developer ID App Attest provisioning profile path}"
cp "$ANYWHERE_PROVISIONING_PROFILE" "$APP/Contents/embedded.provisionprofile"
SIGN=(--force --options runtime --timestamp --sign "$ANYWHERE_SIGNING_IDENTITY")
if [[ -n "${ANYWHERE_SIGNING_KEYCHAIN:-}" ]]; then
  SIGN+=(--keychain "$ANYWHERE_SIGNING_KEYCHAIN")
fi
FRAMEWORK="$APP/Contents/Frameworks/Sparkle.framework"
if [[ -d "$FRAMEWORK" ]]; then
  while IFS= read -r -d '' component; do
    codesign "${SIGN[@]}" --preserve-metadata=entitlements "$component"
  done < <(find "$FRAMEWORK/Versions/B" -depth \( -name '*.xpc' -o -name '*.app' -o -name Autoupdate \) -print0)
  codesign "${SIGN[@]}" "$FRAMEWORK"
fi
for binary in anywhere-host anywhere-permission-probe; do
  codesign "${SIGN[@]}" "$APP/Contents/MacOS/$binary"
done
codesign "${SIGN[@]}" --entitlements "$ROOT/Resources/AnywhereHost.entitlements" "$APP"
python3 "$ROOT/Scripts/verify-app.py" "$APP"
