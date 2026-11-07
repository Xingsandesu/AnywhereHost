#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:?Usage: build-native-touch-probe.sh /absolute/output/AnywhereTouchProbe.app}"
if [[ "$OUT" != /* || "$OUT" != *.app || -e "$OUT" ]]; then
  echo 'Output must be a new absolute .app path.' >&2
  exit 2
fi
mkdir -p "$OUT/Contents/MacOS"
xcrun swiftc -parse-as-library -framework AppKit -framework CoreHID \
  "$ROOT/Sources/AnywhereHost/Desktop/DesktopTouchReport.swift" \
  "$ROOT/Sources/AnywhereHost/Desktop/DesktopNativeTouch.swift" \
  "$ROOT/Sources/AnywhereHost/Desktop/DesktopVirtualDisplay.swift" \
  "$ROOT/Sources/AnywhereHost/Desktop/DesktopNativeTouchProbe.swift" \
  "$ROOT/Scripts/NativeTouchStreamProbe.swift" \
  "$ROOT/Scripts/NativeTouchProbeMain.swift" \
  -o "$OUT/Contents/MacOS/AnywhereTouchProbe"
python3 - "$OUT" <<'PY'
import pathlib,plistlib,sys
out=pathlib.Path(sys.argv[1])
(out/'Contents/Info.plist').write_bytes(plistlib.dumps({
 'CFBundleIdentifier':'com.jintongshu.AnywhereHost.TouchProbe',
 'CFBundleExecutable':'AnywhereTouchProbe','CFBundleName':'Anywhere Touch Probe',
 'CFBundlePackageType':'APPL','CFBundleVersion':'1','CFBundleShortVersionString':'1.0',
 'LSMinimumSystemVersion':'27.0','NSIsTouchNative':True}))
# This is a signing input, NOT a grant of the entitlement.
(out.parent/(out.stem+'.entitlements')).write_bytes(plistlib.dumps({'com.apple.developer.hid.virtual.device':True}))
PY
# An ordinary ad-hoc build can run and report the authorization error. Do not
# add restricted entitlements to an executable that cannot be authorized to run.
codesign --force --sign - "$OUT"
printf 'Probe: %s/Contents/MacOS/AnywhereTouchProbe\n' "$OUT"
printf 'HID signing input: %s.entitlements\n' "${OUT%.app}"
