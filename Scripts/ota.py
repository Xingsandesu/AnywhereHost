#!/usr/bin/env python3
"""Prepare and independently verify Sparkle OTA release metadata."""
import argparse
import base64
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
import zipfile

ROOT = Path(__file__).resolve().parents[1]
NAMESPACE = 'http://www.andymatuschak.org/xml-namespaces/sparkle'
MINIMUM_BUILD = 8  # The 0.1.5 update identity starts here; do not increase per release.
PUBLIC_DER_PREFIX = bytes.fromhex('302a300506032b6570032100')


def public_key():
    info = plistlib.loads((ROOT / 'Resources/Info.plist').read_bytes())
    raw = base64.b64decode(info['SUPublicEDKey'], validate=True)
    if len(raw) != 32:
        raise ValueError('Bundled OTA public key must be 32 bytes')
    return raw


def verify_private_seed(seed_text, directory, expected_public):
    try:
        seed = base64.b64decode(seed_text.strip(), validate=True)
    except ValueError:
        raise ValueError('SPARKLE_PRIVATE_KEY must contain a Base64 Ed25519 seed') from None
    if len(seed) != 32:
        raise ValueError('SPARKLE_PRIVATE_KEY must decode to a 32-byte Ed25519 seed')
    directory = Path(directory)
    der = directory / 'key.der'
    # RFC 8410 PKCS#8 wrapper around the Ed25519 seed.
    with os.fdopen(os.open(der, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), 'wb') as stream:
        stream.write(bytes.fromhex('302e020100300506032b657004220420') + seed)
    derived = subprocess.check_output(['openssl', 'pkey', '-inform', 'DER', '-in', str(der), '-pubout', '-outform', 'DER'])
    if derived != PUBLIC_DER_PREFIX + expected_public:
        raise ValueError('OTA secret does not match the public key bundled in the app')


def app_info(archive):
    with zipfile.ZipFile(archive) as bundle:
        return plistlib.loads(bundle.read('AnywhereHost.app/Contents/Info.plist'))


def verify_signature(path, encoded_signature, raw_public):
    signature = base64.b64decode(encoded_signature.strip(), validate=True)
    if len(signature) != 64:
        raise ValueError('Invalid Ed25519 signature length')
    with tempfile.TemporaryDirectory(prefix='anywhere-ota-verify-') as temporary:
        key, sig = Path(temporary) / 'public.der', Path(temporary) / 'signature'
        key.write_bytes(PUBLIC_DER_PREFIX + raw_public)
        sig.write_bytes(signature)
        result = subprocess.run(['openssl', 'pkeyutl', '-verify', '-pubin', '-keyform', 'DER',
                                 '-inkey', str(key), '-rawin', '-in', str(path), '-sigfile', str(sig)],
                                capture_output=True)
        if result.returncode:
            raise ValueError('OTA signature verification failed: ' + Path(path).name)


def verify(metadata, archive, tag):
    metadata, archive = Path(metadata), Path(archive)
    public = public_key()
    info = app_info(archive)
    version, build = info['CFBundleShortVersionString'], info['CFBundleVersion']
    if tag != 'v' + version or not re.fullmatch(r'\d+\.\d+\.\d+', version) or not build.isdecimal() or int(build) < MINIMUM_BUILD:
        raise ValueError('Invalid stable release version/build')
    if base64.b64decode(info['SUPublicEDKey'], validate=True) != public:
        raise ValueError('Mac bundle uses a different OTA public key')
    if info['SUFeedURL'] != 'https://github.com/Xingsandesu/AnywhereHost/releases/latest/download/appcast.xml':
        raise ValueError('Unexpected Sparkle update feed')
    root = ET.parse(metadata / 'appcast.xml').getroot()
    items = root.findall('./channel/item')
    if len(items) != 1:
        raise ValueError('Expected one current release in the appcast')
    item = items[0]
    field = lambda name: item.findtext('{' + NAMESPACE + '}' + name)
    enclosure = item.find('enclosure')
    if field('version') != build or field('shortVersionString') != version or field('minimumUpdateVersion') != str(MINIMUM_BUILD):
        raise ValueError('Appcast version or OTA baseline differs from the bundle')
    expected_url = f'https://github.com/Xingsandesu/AnywhereHost/releases/download/{tag}/{archive.name}'
    if enclosure is None or enclosure.get('url') != expected_url or enclosure.get('length') != str(archive.stat().st_size):
        raise ValueError('Appcast archive URL/size mismatch')
    verify_signature(archive, enclosure.get('{' + NAMESPACE + '}edSignature', ''), public)
    print('PASS Sparkle appcast, bundle identity, OTA baseline and Ed25519 signature')


def sign(archive, output):
    archive, output = Path(archive).resolve(), Path(output).resolve()
    secret = os.environ.pop('SPARKLE_PRIVATE_KEY', '')
    if not secret:
        raise ValueError('Missing GitHub secret SPARKLE_PRIVATE_KEY')
    tools = ROOT / '.build/artifacts/sparkle/Sparkle/bin'
    if not (tools / 'generate_appcast').is_file():
        raise ValueError('Run swift package resolve to obtain the pinned Sparkle tools')
    version = app_info(archive)['CFBundleShortVersionString']
    if not re.fullmatch(r'\d+\.\d+\.\d+', version):
        raise ValueError('Invalid app version')
    tag = 'v' + version
    output.mkdir(parents=True, exist_ok=False)
    with tempfile.TemporaryDirectory(prefix='anywhere-ota-sign-', dir=os.environ.get('RUNNER_TEMP')) as temporary:
        private = Path(temporary)
        verify_private_seed(secret, private, public_key())
        seed_file = private / 'sparkle.key'
        with os.fdopen(os.open(seed_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), 'w') as stream:
            stream.write(secret.strip())
        del secret
        updates = private / 'updates'
        updates.mkdir()
        shutil.copyfile(archive, updates / archive.name)
        subprocess.run([str(tools / 'generate_appcast'), '--ed-key-file', str(seed_file),
                        '--maximum-deltas', '0', '--maximum-versions', '1',
                        '--minimum-update-version', str(MINIMUM_BUILD),
                        '--download-url-prefix', f'https://github.com/Xingsandesu/AnywhereHost/releases/download/{tag}/',
                        str(updates)], check=True)
        shutil.copyfile(updates / 'appcast.xml', output / 'appcast.xml')
    verify(output, archive, tag)
    # CryptoKit independently checks the Sparkle signature on the Mac runner.
    subprocess.run([sys.executable, str(ROOT / 'Scripts/verify-update.py'),
                    str(output / 'appcast.xml'), str(archive)], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=('sign', 'verify'))
    parser.add_argument('archive', type=Path)
    parser.add_argument('metadata', type=Path)
    parser.add_argument('--tag')
    args = parser.parse_args()
    if args.command == 'sign':
        sign(args.archive, args.metadata)
    else:
        verify(args.metadata, args.archive, args.tag)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
