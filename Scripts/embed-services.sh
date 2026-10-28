#!/bin/bash
set -euo pipefail
cd "$SRCROOT"
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"

if [[ "$ARCHS" != "arm64" || "$(uname -m)" != "arm64" ]]; then
  printf 'Host packaging currently requires an Apple Silicon build host and arm64 target.\n' >&2
  exit 1
fi

python3 Scripts/fetch-frpc.py
cargo build --release --locked

HOST_EXECUTABLES="$TARGET_BUILD_DIR/$EXECUTABLE_FOLDER_PATH"
HOST_RESOURCES="$TARGET_BUILD_DIR/$UNLOCALIZED_RESOURCES_FOLDER_PATH"
mkdir -p "$HOST_EXECUTABLES" "$HOST_RESOURCES"
cp target/release/anywhere-host "$HOST_EXECUTABLES/anywhere-host"
cp Resources/frpc "$HOST_EXECUTABLES/frpc"
cp Resources/frp-LICENSE "$HOST_RESOURCES/frp-LICENSE"
if [[ "${CODE_SIGNING_ALLOWED:-YES}" != "NO" && -n "${EXPANDED_CODE_SIGN_IDENTITY:-}" ]]; then
  for helper in anywhere-host frpc; do
    /usr/bin/codesign --force --options runtime --entitlements "$SRCROOT/Resources/InheritedSandbox.entitlements" --sign "$EXPANDED_CODE_SIGN_IDENTITY" "$HOST_EXECUTABLES/$helper"
  done
fi
