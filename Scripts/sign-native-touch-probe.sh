#!/bin/bash
set -euo pipefail
APP="${1:?Usage: sign-native-touch-probe.sh App.app profile.provisionprofile certificate-SHA1}"
PROFILE="${2:?Missing provisioning profile}"
IDENTITY="${3:?Missing signing certificate SHA1}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
security cms -D -i "$PROFILE" > "$WORK/profile.plist"
python3 - "$APP" "$WORK/profile.plist" "$IDENTITY" "$WORK/entitlements.plist" <<'PY'
import datetime,hashlib,pathlib,plistlib,sys
app,profile,identity,output=sys.argv[1:]
info=plistlib.loads((pathlib.Path(app)/'Contents/Info.plist').read_bytes())
p=plistlib.loads(pathlib.Path(profile).read_bytes())
ent=p['Entitlements']
identifier=info['CFBundleIdentifier']
team=ent.get('com.apple.developer.team-identifier')
expected=f'{team}.{identifier}'
if ent.get('com.apple.developer.hid.virtual.device') is not True:
    raise SystemExit('Profile does not grant com.apple.developer.hid.virtual.device. Request HID Virtual Device, then regenerate the profile.')
if not team or ent.get('com.apple.application-identifier') != expected:
    raise SystemExit('Profile does not match the probe App ID.')
if p['ExpirationDate'].replace(tzinfo=datetime.timezone.utc) <= datetime.datetime.now(datetime.timezone.utc):
    raise SystemExit('Provisioning profile has expired.')
certificates={hashlib.sha1(c).hexdigest().upper() for c in p['DeveloperCertificates']}
if identity.upper() not in certificates:
    raise SystemExit('Signing certificate is not included in this profile. Supply its SHA1 fingerprint.')
keys=('com.apple.application-identifier','com.apple.developer.team-identifier',
      'com.apple.developer.hid.virtual.device','com.apple.security.get-task-allow','get-task-allow')
pathlib.Path(output).write_bytes(plistlib.dumps({k:ent[k] for k in keys if k in ent}))
PY
cp "$PROFILE" "$APP/Contents/embedded.provisionprofile"
codesign --force --sign "$IDENTITY" --entitlements "$WORK/entitlements.plist" "$APP"
codesign --verify --strict --verbose=2 "$APP"
printf 'Signed probe: %s/Contents/MacOS/AnywhereTouchProbe\n' "$APP"
printf 'Run the probe and require AppKit direct-contact PASS before enabling native touch.\n'
