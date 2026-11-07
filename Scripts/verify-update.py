#!/usr/bin/env python3
"""Verify appcast metadata and the update's Ed25519 signature without private keys."""
import base64
import argparse
import pathlib
import plistlib
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
import zipfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('feed', type=pathlib.Path)
parser.add_argument('archive', type=pathlib.Path)
parser.add_argument('--previous-app', type=pathlib.Path, help='Verify a key rotation against an installed signed release')
args = parser.parse_args()
feed, archive = args.feed, args.archive
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
    if args.previous_app:
        previous = args.previous_app.resolve()
        previous_info = plistlib.loads((previous / 'Contents/Info.plist').read_bytes())
        assert previous_info['CFBundleIdentifier'] == info['CFBundleIdentifier'], 'Application identity changed'
        assert int(info['CFBundleVersion']) > int(previous_info['CFBundleVersion']), 'Update build must increase'
        if previous_info['SUPublicEDKey'] != info['SUPublicEDKey']:
            assert not previous_info.get('SUVerifyUpdateBeforeExtraction', False), 'Key rotation requires a Developer ID signed disk image for this previous app'
        subprocess.run(['ditto', '-x', '-k', str(archive), folder], check=True)
        updated = pathlib.Path(folder) / 'AnywhereHost.app'
        subprocess.run(['codesign', '--verify', '--deep', '--strict', str(updated)], check=True)
        # Sparkle 2.10.1 SUCodeSigningVerifier uses this designated requirement
        # check for key rotation. A renewed certificate in the same team can match.
        source.write_text('''import Foundation
import Security
var previous: SecStaticCode?
var updated: SecStaticCode?
var requirement: SecRequirement?
guard SecStaticCodeCreateWithPath(URL(fileURLWithPath: CommandLine.arguments[1]) as CFURL, [], &previous) == errSecSuccess,
      SecStaticCodeCreateWithPath(URL(fileURLWithPath: CommandLine.arguments[2]) as CFURL, [], &updated) == errSecSuccess,
      let previous, let updated,
      SecCodeCopyDesignatedRequirement(previous, [], &requirement) == errSecSuccess,
      let requirement else { exit(1) }
var error: Unmanaged<CFError>?
guard SecStaticCodeCheckValidityWithErrors(updated, SecCSFlags(rawValue: kSecCSCheckAllArchitectures), requirement, &error) == errSecSuccess else {
    if let error { fputs("\\(error.takeRetainedValue())\\n", stderr) }
    exit(1)
}
''')
        subprocess.run(['xcrun', 'swift', str(source), str(previous), str(updated)], check=True)
        print('PASS previous-release designated requirement and Sparkle key rotation trust')
print('PASS appcast URL, bundle version, archive size and Ed25519 signature')
