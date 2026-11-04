#!/bin/bash
set -euo pipefail
cd "$SRCROOT"

if [[ "$ARCHS" != "arm64" || "$(uname -m)" != "arm64" ]]; then
  printf 'Host packaging currently requires an Apple Silicon build host and arm64 target.\n' >&2
  exit 1
fi

HOST_EXECUTABLES="$TARGET_BUILD_DIR/$EXECUTABLE_FOLDER_PATH"
HOST_RESOURCES="$TARGET_BUILD_DIR/$UNLOCALIZED_RESOURCES_FOLDER_PATH"
mkdir -p "$HOST_EXECUTABLES" "$HOST_RESOURCES"
python3 - "$HOST_EXECUTABLES" <<'PY'
from pathlib import Path
import shutil, sys
sys.path.insert(0, 'Scripts')
from cargo_artifacts import build
artifacts = build('--release', '--locked', '-p', 'anywhere-host', '--bins')
for name in ('anywhere-host', 'anywhere-permission-probe'):
    shutil.copy2(artifacts[name]['executable'], Path(sys.argv[1]) / name)
PY
cp LICENSE NOTICE "$HOST_RESOURCES/"
if [[ "${CODE_SIGNING_ALLOWED:-YES}" != "NO" && -n "${EXPANDED_CODE_SIGN_IDENTITY:-}" ]]; then
  for helper in anywhere-host anywhere-permission-probe; do
    /usr/bin/codesign --force --options runtime --sign "$EXPANDED_CODE_SIGN_IDENTITY" "$HOST_EXECUTABLES/$helper"
  done
fi
