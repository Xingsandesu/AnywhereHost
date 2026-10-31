#!/usr/bin/env python3
"""Verify appcast metadata and the update's Ed25519 signature without private keys."""
import base64
import pathlib
import plistlib
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
import zipfile

feed, archive = map(pathlib.Path, sys.argv[1:])
ns = {'s': 'http://www.andymatuschak.org/xml-namespaces/sparkle'}
item = ET.parse(feed).getroot().find('./channel/item')
assert item is not None, 'Missing release'
enclosure = item.find('enclosure')
assert enclosure is not None
assert int(enclosure.attrib['length']) == archive.stat().st_size
assert enclosure.attrib['url'] == 'https://github.com/Xingsandesu/AnywhereHost/releases/download/v' + item.findtext('s:shortVersionString', namespaces=ns) + '/' + archive.name
with zipfile.ZipFile(archive) as z:
    info = plistlib.loads(z.read('AnywhereHost.app/Contents/Info.plist'))
expected = plistlib.loads((pathlib.Path(__file__).resolve().parents[1] / 'Resources/Info.plist').read_bytes())
assert info['SUPublicEDKey'] == expected['SUPublicEDKey'], 'Untrusted update signing key'
assert item.findtext('s:version', namespaces=ns) == info['CFBundleVersion']
assert item.findtext('s:shortVersionString', namespaces=ns) == info['CFBundleShortVersionString']
assert info['SUFeedURL'] == 'https://github.com/Xingsandesu/AnywhereHost/releases/latest/download/appcast.xml'
# CryptoKit is part of the macOS SDK; no third-party Python crypto dependency.
with tempfile.TemporaryDirectory(prefix='anywhere-update-verify-') as folder:
    source = pathlib.Path(folder) / 'verify.swift'
    source.write_text('''import Foundation
import CryptoKit
let key = try Curve25519.Signing.PublicKey(rawRepresentation: Data(base64Encoded: CommandLine.arguments[1])!)
let signature = Data(base64Encoded: CommandLine.arguments[2])!
let data = try Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[3]))
guard key.isValidSignature(signature, for: data) else { exit(1) }
''')
    signature = enclosure.attrib['{' + ns['s'] + '}edSignature']
    assert len(base64.b64decode(signature)) == 64
    subprocess.run(['xcrun', 'swift', str(source), info['SUPublicEDKey'], signature, str(archive)], check=True)
print('PASS appcast URL, bundle version, archive size and Ed25519 signature')
