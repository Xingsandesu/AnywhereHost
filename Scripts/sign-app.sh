#!/bin/bash
# Sign an assembled app, including Sparkle's nested code, from the inside out.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[[ "$#" == 1 && -d "$1/Contents" ]] || { printf 'Usage: Scripts/sign-app.sh APP\n' >&2; exit 1; }
: "${ANYWHERE_SIGNING_IDENTITY:?Set a real signing identity}"
[[ "$ANYWHERE_SIGNING_IDENTITY" != - ]] || { printf 'Ad-hoc signing is not supported.\n' >&2; exit 1; }
APP="$1"
: "${ANYWHERE_PROVISIONING_PROFILE:?Set the Developer ID App Attest provisioning profile path}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
security cms -D -i "$ANYWHERE_PROVISIONING_PROFILE" > "$WORK/profile.plist"
python3 - "$ROOT/Resources/AnywhereHost.entitlements" "$WORK/profile.plist" "$WORK/entitlements.plist" <<'PY'
import os,pathlib,plistlib,sys
base,profile,output=map(pathlib.Path,sys.argv[1:])
ent=plistlib.loads(base.read_bytes())
p=plistlib.loads(profile.read_bytes())
granted=p['Entitlements']
development=os.environ.get('ANYWHERE_DEVELOPMENT_SIGNING')=='1'
if not p.get('ProvisionsAllDevices') and not development:
    raise SystemExit('Development profiles require explicit ANYWHERE_DEVELOPMENT_SIGNING=1; this is not a distribution build.')
if granted.get('com.apple.developer.hid.virtual.device') is True:
    ent['com.apple.developer.hid.virtual.device']=True
if development:
    for key in ('get-task-allow','com.apple.security.get-task-allow'):
        if key in granted: ent[key]=granted[key]
output.write_bytes(plistlib.dumps(ent))
PY
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
# Xcode debug builds load the app code from a sibling dylib.
shopt -s nullglob
for library in "$APP/Contents/MacOS/"*.dylib; do
  codesign "${SIGN[@]}" "$library"
done
codesign "${SIGN[@]}" --entitlements "$WORK/entitlements.plist" "$APP"
VERIFY_OPTIONS=()
if [[ "${ANYWHERE_DEVELOPMENT_SIGNING:-0}" == 1 ]]; then VERIFY_OPTIONS+=(--development); fi
python3 "$ROOT/Scripts/verify-app.py" "$APP" "${VERIFY_OPTIONS[@]}"
