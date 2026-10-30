#!/usr/bin/env python3
"""Verify direct-distribution signatures; optionally run the packaged Host fixture."""
import argparse
import pathlib
import plistlib
import subprocess
import sys

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('app', type=pathlib.Path)
parser.add_argument('--integration', action='store_true')
args = parser.parse_args()
app = args.app.resolve()
subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)], check=True)
assert not (app / 'Contents/embedded.provisionprofile').exists(), 'Unexpected store provisioning profile'
for name in ('AnywhereHost', 'anywhere-host', 'anywhere-permission-probe', 'frpc'):
    executable = app / 'Contents/MacOS' / name
    subprocess.run(['codesign', '--verify', '--strict', str(executable)], check=True)
    signature = subprocess.run(['codesign', '-d', '--verbose=4', str(executable)], capture_output=True, check=True)
    assert b'runtime' in signature.stderr, f'{name} must retain Hardened Runtime'
    result = subprocess.run(['codesign', '-d', '--entitlements', ':-', str(executable)], capture_output=True, check=True)
    entitlements = plistlib.loads(result.stdout) if result.stdout.strip() else {}
    assert not entitlements.get('com.apple.security.app-sandbox'), f'{name} still has App Sandbox'
    assert not entitlements.get('com.apple.security.inherit'), f'{name} still inherits App Sandbox'
    if name == 'AnywhereHost':
        assert entitlements.get('com.apple.security.automation.apple-events'), 'Host is missing Apple Events consent entitlement'
        info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
        assert info.get('NSAppleEventsUsageDescription'), 'Missing Apple Events usage description'
    print(f'PASS {name}: valid signature, Hardened Runtime, no App Sandbox', flush=True)
if args.integration:
    subprocess.run([sys.executable, str(pathlib.Path(__file__).with_name('verify-host.py')),
                    '--binary', str(app / 'Contents/MacOS/anywhere-host')], check=True)
