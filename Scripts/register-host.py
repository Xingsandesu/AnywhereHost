#!/usr/bin/env python3
"""登记 GitHub 官方流水线验证的 macOS 公证产物，使用短期 OIDC 身份。"""
import argparse
import json
import os
from pathlib import Path
import plistlib
import re
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('app', type=Path)
parser.add_argument('source_sha')
args = parser.parse_args()
app = args.app.resolve()
assert re.fullmatch('[a-f0-9]{40}', args.source_sha), 'Invalid source commit'
subprocess.run([sys.executable, str(Path(__file__).with_name('verify-app.py')), str(app)], check=True)
subprocess.run(['spctl', '--assess', '--type', 'execute', '--verbose=2', str(app)], check=True)
info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
assert info['CFBundleIdentifier'] == 'com.jintongshu.AnywhereHost', 'Unexpected application'
assert re.fullmatch(r'\d{1,8}', info['CFBundleVersion']) and int(info['CFBundleVersion']) >= 51, 'Unsupported build'
assert re.fullmatch(r'\d+\.\d+\.\d+', info['CFBundleShortVersionString']), 'Invalid version'
signature = subprocess.run(['codesign', '-d', '--verbose=4', str(app)], check=True, capture_output=True, text=True).stderr
assert '\nTeamIdentifier=775Y7KV7KU\n' in signature, 'Unexpected signer'
hashes = re.findall(r'^CandidateCDHashFull sha256=([a-f0-9]{64})$', signature, re.MULTILINE)
assert len(hashes) == 1, 'Unique SHA-256 CodeDirectory hash required'
url = 'https://connect.jintongshu.com/v3/network/releases'
identity_url = os.environ['ACTIONS_ID_TOKEN_REQUEST_URL']
assert urllib.parse.urlsplit(identity_url).scheme == 'https', 'HTTPS identity endpoint required'
identity_url += ('&' if '?' in identity_url else '?') + urllib.parse.urlencode({'audience': url})
identity_request = urllib.request.Request(identity_url, headers={'Authorization': 'Bearer ' + os.environ['ACTIONS_ID_TOKEN_REQUEST_TOKEN']})
with urllib.request.urlopen(identity_request, timeout=30) as response:
    identity = json.load(response)['value']
release = {'codeHash': hashes[0], 'build': int(info['CFBundleVersion']), 'version': info['CFBundleShortVersionString'], 'sourceSha': args.source_sha}
request = urllib.request.Request(url, method='POST', data=json.dumps(release).encode(), headers={'Content-Type': 'application/json', 'Authorization': 'Bearer ' + identity, 'User-Agent': 'AnywhereHost-Release/1.1.1'})
try:
    with urllib.request.urlopen(request, timeout=30) as response:
        result = json.load(response)
except urllib.error.HTTPError as error:
    payload = error.read(4096)
    try:
        detail = json.loads(payload).get('error', 'RegistrationFailed')
    except (ValueError, AttributeError):
        detail = 'Non-JSON response from edge; content-type=' + error.headers.get('Content-Type', '')
    raise SystemExit(f'Host registration failed: HTTP {error.code}, {detail}') from None
assert result == {'registered': True, 'codeHash': hashes[0]}, 'Registration not confirmed'
print('Registered official Host build:', json.dumps(release))
