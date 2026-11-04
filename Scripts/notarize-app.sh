#!/bin/bash
# Only output the final archive after Apple accepts, staples and assesses it.
set +x
set -euo pipefail
[[ "$#" == 2 ]] || { printf 'Usage: Scripts/notarize-app.sh APP OUTPUT.zip\n' >&2; exit 1; }
: "${APPLE_ID:?Missing GitHub secret APPLE_ID}"
: "${APPLE_APP_SPECIFIC_PASSWORD:?Missing GitHub secret APPLE_APP_SPECIFIC_PASSWORD}"
: "${APPLE_TEAM_ID:?Missing GitHub secret APPLE_TEAM_ID}"
APP="$1"
OUTPUT="$2"
[[ ! -e "$OUTPUT" ]] || { printf 'Output already exists: %s\n' "$OUTPUT" >&2; exit 1; }
umask 077
PRIVATE_DIR=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/anywhere-notary.XXXXXX")
KEYCHAIN="$PRIVATE_DIR/notary.keychain-db"
cleanup() {
  security delete-keychain "$KEYCHAIN" >/dev/null 2>&1 || true
  rm -rf "$PRIVATE_DIR"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
KEYCHAIN_PASSWORD=$(openssl rand -hex 32)
security create-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
security set-keychain-settings -lut 3600 "$KEYCHAIN"
xcrun notarytool store-credentials anywhere-host --keychain "$KEYCHAIN" \
  --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_SPECIFIC_PASSWORD" >/dev/null
unset APPLE_APP_SPECIFIC_PASSWORD
ditto -c -k --sequesterRsrc --keepParent "$APP" "$PRIVATE_DIR/submission.zip"
# Keep the JSON response even if Apple's tool exits nonzero for a rejection.
status=0
xcrun notarytool submit "$PRIVATE_DIR/submission.zip" --keychain "$KEYCHAIN" \
  --keychain-profile anywhere-host --wait --timeout 30m --output-format json > "$PRIVATE_DIR/result.json" || status=$?
if ! python3 - "$PRIVATE_DIR/result.json" <<'PY'
import json, pathlib, sys
result = json.loads(pathlib.Path(sys.argv[1]).read_text())
print('Apple notarization:', result.get('status', 'unknown'), 'submission:', result.get('id', 'unknown'))
sys.exit(0 if result.get('status') == 'Accepted' else 1)
PY
then
  submission=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("id", ""))' "$PRIVATE_DIR/result.json")
  if [[ -n "$submission" ]]; then
    xcrun notarytool log "$submission" --keychain "$KEYCHAIN" --keychain-profile anywhere-host || true
  fi
  exit 1
fi
[[ "$status" == 0 ]] || exit "$status"
xcrun stapler staple "$APP"
xcrun stapler validate "$APP"
codesign --verify --deep --strict "$APP"
spctl --assess --type execute --verbose=2 "$APP"
mkdir -p "$(dirname "$OUTPUT")"
ditto -c -k --sequesterRsrc --keepParent "$APP" "$OUTPUT"
printf 'PASS notarized and stapled archive: %s\n' "$OUTPUT"
