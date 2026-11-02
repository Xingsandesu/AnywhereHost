#!/bin/bash
# Import the GitHub certificate into an isolated, short-lived keychain.
set +x
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[[ "$#" == 1 ]] || { printf 'Usage: Scripts/ci-sign-macos.sh APP\n' >&2; exit 1; }
: "${APPLE_CERTIFICATE_P12_BASE64:?Missing GitHub secret APPLE_CERTIFICATE_P12_BASE64}"
: "${APPLE_CERTIFICATE_PASSWORD:?Missing GitHub secret APPLE_CERTIFICATE_PASSWORD}"
: "${APPLE_TEAM_ID:?Missing GitHub secret APPLE_TEAM_ID}"
umask 077
PRIVATE_DIR=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/anywhere-sign.XXXXXX")
KEYCHAIN="$PRIVATE_DIR/signing.keychain-db"
cleanup() {
  security delete-keychain "$KEYCHAIN" >/dev/null 2>&1 || true
  rm -rf "$PRIVATE_DIR"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
python3 - "$PRIVATE_DIR" <<'PY'
import base64, hashlib, os, pathlib, sys, urllib.request
directory = pathlib.Path(sys.argv[1])
(directory / 'identity.p12').write_bytes(base64.b64decode(os.environ['APPLE_CERTIFICATE_P12_BASE64'], validate=True))
# Public Apple intermediates; the root remains the OS trust anchor.
certificates = {
    'DeveloperIDG2CA.cer': 'f16cd3c54c7f83cea4bf1a3e6a0819c8aaa8e4a1528fd144715f350643d2df3a',
    'DeveloperIDCA.cer': '7afc9d01a62f03a2de9637936d4afe68090d2de18d03f29c88cfb0b1ba63587f',
}
for name, digest in certificates.items():
    with urllib.request.urlopen('https://www.apple.com/certificateauthority/' + name, timeout=30) as response:
        data = response.read()
    if hashlib.sha256(data).hexdigest() != digest:
        raise SystemExit('Apple intermediate checksum mismatch: ' + name)
    (directory / name).write_bytes(data)
PY
KEYCHAIN_PASSWORD=$(openssl rand -hex 32)
security create-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
security set-keychain-settings -lut 3600 "$KEYCHAIN"
security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
for certificate in "$PRIVATE_DIR"/*.cer; do
  security import "$certificate" -k "$KEYCHAIN" >/dev/null
done
security import "$PRIVATE_DIR/identity.p12" -k "$KEYCHAIN" -P "$APPLE_CERTIFICATE_PASSWORD" -T /usr/bin/codesign >/dev/null
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KEYCHAIN_PASSWORD" "$KEYCHAIN" >/dev/null
ANYWHERE_SIGNING_IDENTITY=$(security find-identity -v -p codesigning "$KEYCHAIN" | python3 -c '
import os, re, sys
rows = re.findall(r"([A-F0-9]{40}) \"([^\"]+)\"", sys.stdin.read())
matches = [h for h, name in rows if name.startswith("Developer ID Application:") and name.endswith("(" + os.environ["APPLE_TEAM_ID"] + ")")]
if len(matches) != 1:
    sys.exit("Expected exactly one valid Developer ID Application identity for APPLE_TEAM_ID")
print(matches[0])
')
export ANYWHERE_SIGNING_IDENTITY
export ANYWHERE_SIGNING_KEYCHAIN="$KEYCHAIN"
unset APPLE_CERTIFICATE_P12_BASE64 APPLE_CERTIFICATE_PASSWORD
bash "$ROOT/Scripts/sign-app.sh" "$1"
codesign -d --verbose=4 "$1" 2> "$PRIVATE_DIR/signature.txt"
python3 - "$PRIVATE_DIR/signature.txt" <<'PY'
import os, pathlib, sys
lines = pathlib.Path(sys.argv[1]).read_text().splitlines()
assert 'TeamIdentifier=' + os.environ['APPLE_TEAM_ID'] in lines, 'Unexpected signing team'
assert any(line.startswith('Authority=Developer ID Application:') for line in lines), 'Wrong certificate type'
assert any(line.startswith('Timestamp=') for line in lines), 'Missing secure timestamp'
print('PASS Developer ID Application, expected team, secure timestamp')
PY
