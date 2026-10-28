#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"
python3 Scripts/fetch-frpc.py
cargo build --release --locked
swift build -c release
APP="$ROOT/dist/AnywhereHost.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp .build/release/AnywhereHost "$APP/Contents/MacOS/AnywhereHost"
cp target/release/anywhere-host "$APP/Contents/MacOS/anywhere-host"
cp Resources/frpc "$APP/Contents/MacOS/frpc"
cp Resources/frp-LICENSE "$APP/Contents/Resources/frp-LICENSE"
cp Resources/Info.plist "$APP/Contents/Info.plist"
for binary in AnywhereHost anywhere-host frpc; do
  codesign --force --sign "${ANYWHERE_SIGNING_IDENTITY:--}" "$APP/Contents/MacOS/$binary"
done
codesign --force --sign "${ANYWHERE_SIGNING_IDENTITY:--}" "$APP"
codesign --verify --deep --strict "$APP"
printf 'Built %s\n' "$APP"
