#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/../.."
SOURCE_APP="${1:?Pass a built sandboxed AnywhereHost.app}"
APP="$PWD/.build/SandboxVerification.app"
mkdir -p "$APP/Contents/MacOS"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleExecutable</key><string>SandboxVerification</string><key>CFBundleIdentifier</key><string>com.jintongshu.AnywhereHost.SandboxVerification</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleName</key><string>Host Sandbox Verification</string></dict></plist>
PLIST
swiftc -swift-version 6 Sources/AnywhereHost/HostClient.swift Sources/AnywhereHost/HostPermissions.swift Sources/AnywhereHost/HostProcess.swift Scripts/SandboxVerification/main.swift -o "$APP/Contents/MacOS/SandboxVerification"
for helper in anywhere-host frpc; do
  cp "$SOURCE_APP/Contents/MacOS/$helper" "$APP/Contents/MacOS/$helper"
  codesign --force --options runtime --entitlements Resources/InheritedSandbox.entitlements --sign - "$APP/Contents/MacOS/$helper"
done
codesign --force --options runtime --entitlements Resources/AnywhereHost.entitlements --sign - "$APP"
codesign --verify --deep --strict "$APP"
printf '%s\n' "$APP"
