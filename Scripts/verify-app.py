#!/usr/bin/env python3
"""Verify a signed distribution or explicitly unsigned CI bundle and optional Host fixture."""
import argparse
import os
import pathlib
import plistlib
import subprocess
import sys

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('app', type=pathlib.Path)
parser.add_argument('--integration', action='store_true')
parser.add_argument('--unsigned', action='store_true', help='Check CI bundle structure/architecture only; no signing assurance')
args = parser.parse_args()
app = args.app.resolve()
if not args.unsigned:
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)], check=True)
assert (app / 'Contents/Frameworks/Sparkle.framework').exists(), 'Missing OTA framework'
assert not (app / 'Contents/embedded.provisionprofile').exists(), 'Unexpected store provisioning profile'
assert (app / 'Contents/Resources/frp-LICENSE').is_file(), 'Missing frp license'
assert not list((app / 'Contents/MacOS').glob('*codex*')), 'Codex must not be redistributed'
info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
assert info.get('NSAppleEventsUsageDescription'), 'Missing Apple Events usage description'
assert info.get('CFBundleShortVersionString') and info.get('CFBundleVersion'), 'Missing version'
assert info.get('SUPublicEDKey') and info.get('SUFeedURL', '').startswith('https://'), 'Missing signed OTA configuration'
backend_version = subprocess.check_output([str(app / 'Contents/MacOS/anywhere-host'), '--version'], text=True).strip()
assert backend_version == 'AnywhereHost ' + info['CFBundleShortVersionString'], 'App/backend version mismatch'
for name in ('AnywhereHost', 'anywhere-host', 'anywhere-permission-probe', 'frpc'):
    executable = app / 'Contents/MacOS' / name
    assert os.access(executable, os.X_OK), f'{name} is not executable'
    architectures = subprocess.check_output(['lipo', '-archs', str(executable)], text=True).split()
    assert architectures == ['arm64'], f'{name}: expected arm64, got {architectures}'
    if args.unsigned:
        print(f'PASS {name}: executable, arm64 (CI mode; signature not verified)', flush=True)
        continue
    subprocess.run(['codesign', '--verify', '--strict', str(executable)], check=True)
    signature = subprocess.run(['codesign', '-d', '--verbose=4', str(executable)], capture_output=True, check=True)
    assert b'runtime' in signature.stderr, f'{name} must retain Hardened Runtime'
    result = subprocess.run(['codesign', '-d', '--entitlements', ':-', str(executable)], capture_output=True, check=True)
    entitlements = plistlib.loads(result.stdout) if result.stdout.strip() else {}
    assert not entitlements.get('com.apple.security.app-sandbox'), f'{name} still has App Sandbox'
    assert not entitlements.get('com.apple.security.inherit'), f'{name} still inherits App Sandbox'
    if name == 'AnywhereHost':
        assert entitlements.get('com.apple.security.automation.apple-events'), 'Host is missing Apple Events consent entitlement'
        framework_signature = subprocess.run(['codesign', '-d', '--verbose=4', str(app / 'Contents/Frameworks/Sparkle.framework')], capture_output=True, check=True)
        def team(output):
            return next((line for line in output.splitlines() if line.startswith(b'TeamIdentifier=')), b'')
        assert team(signature.stderr) not in (b'', b'TeamIdentifier=not set'), 'Sparkle requires a real signing identity'
        assert team(signature.stderr) == team(framework_signature.stderr), 'Framework team differs from App'
    print(f'PASS {name}: valid signature, Hardened Runtime, no App Sandbox', flush=True)
if args.integration:
    subprocess.run([sys.executable, str(pathlib.Path(__file__).with_name('verify-host.py')),
                    '--binary', str(app / 'Contents/MacOS/anywhere-host')], check=True)
