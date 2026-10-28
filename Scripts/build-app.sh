#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
xcodebuild -project AnywhereHost.xcodeproj -scheme AnywhereHost -configuration Release \
  -destination 'generic/platform=macOS' -derivedDataPath "$ROOT/.build/xcode" \
  CODE_SIGNING_ALLOWED=NO build
APP="$ROOT/dist/AnywhereHost.app"
mkdir -p "$ROOT/dist"
ditto "$ROOT/.build/xcode/Build/Products/Release/AnywhereHost.app" "$APP"
for binary in anywhere-host frpc; do
  codesign --force --options runtime --entitlements Resources/InheritedSandbox.entitlements --sign "${ANYWHERE_SIGNING_IDENTITY:--}" "$APP/Contents/MacOS/$binary"
done
codesign --force --options runtime --entitlements Resources/AnywhereHost.entitlements --sign "${ANYWHERE_SIGNING_IDENTITY:--}" "$APP"
codesign --verify --deep --strict "$APP"
printf 'Built %s\n' "$APP"
