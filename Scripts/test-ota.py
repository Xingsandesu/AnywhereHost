#!/usr/bin/env python3
"""Exercise real Ed25519 signatures and reject broken OTA feeds or payloads."""
import base64
from pathlib import Path
import plistlib
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET
import zipfile

import ota


class OTATests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.key = self.root / 'fixture.pem'
        subprocess.run(['openssl', 'genpkey', '-algorithm', 'ED25519', '-out', str(self.key)], check=True, capture_output=True)
        self.public = subprocess.check_output(['openssl', 'pkey', '-in', str(self.key), '-pubout', '-outform', 'DER'])[-32:]
        der = subprocess.check_output(['openssl', 'pkey', '-in', str(self.key), '-outform', 'DER'])
        self.seed = base64.b64encode(der[-32:]).decode()
        self.archive = self.root / 'AnywhereHost-0.1.5-macos-arm64-notarized.zip'
        info = {'CFBundleShortVersionString': '0.1.5', 'CFBundleVersion': '8',
                'SUPublicEDKey': base64.b64encode(self.public).decode(),
                'SUFeedURL': 'https://github.com/Xingsandesu/AnywhereHost/releases/latest/download/appcast.xml'}
        with zipfile.ZipFile(self.archive, 'w') as bundle:
            bundle.writestr('AnywhereHost.app/Contents/Info.plist', plistlib.dumps(info))
        self.feed = ET.Element('rss')
        item = ET.SubElement(ET.SubElement(self.feed, 'channel'), 'item')
        for field, value in [('version', '8'), ('shortVersionString', '0.1.5'), ('minimumUpdateVersion', '8')]:
            ET.SubElement(item, '{' + ota.NAMESPACE + '}' + field).text = value
        ET.SubElement(item, 'enclosure', {
            'url': 'https://github.com/Xingsandesu/AnywhereHost/releases/download/v0.1.5/' + self.archive.name,
            'length': str(self.archive.stat().st_size), '{' + ota.NAMESPACE + '}edSignature': self.sign(self.archive)})
        self.save_feed()

    def sign(self, path):
        return base64.b64encode(subprocess.check_output(['openssl', 'pkeyutl', '-sign', '-inkey', str(self.key),
                                                         '-rawin', '-in', str(path)])).decode()

    def save_feed(self):
        ET.ElementTree(self.feed).write(self.root / 'appcast.xml')

    def verify(self):
        with patch.object(ota, 'public_key', return_value=self.public):
            ota.verify(self.root, self.archive, 'v0.1.5')

    def test_complete_feed_and_real_signature(self):
        self.verify()

    def test_cli_verifies_real_signature_and_rejects_tampering(self):
        arguments = ['ota.py', 'verify', str(self.archive), str(self.root), '--tag', 'v0.1.5']
        with patch('sys.argv', arguments), patch.object(ota, 'public_key', return_value=self.public):
            ota.main()
            with zipfile.ZipFile(self.archive, 'a') as bundle:
                bundle.writestr('tampered.txt', b'tamper')
            self.feed.find('./channel/item/enclosure').set('length', str(self.archive.stat().st_size))
            self.save_feed()
            with self.assertRaisesRegex(ValueError, 'signature verification failed'):
                ota.main()

    def test_private_seed_must_match_bundled_public_identity(self):
        good = self.root / 'good'
        good.mkdir()
        ota.verify_private_seed(self.seed, good, self.public)
        wrong = self.root / 'wrong'
        wrong.mkdir()
        with self.assertRaisesRegex(ValueError, 'does not match'):
            ota.verify_private_seed(self.seed, wrong, b'\0' * 32)
        with self.assertRaisesRegex(ValueError, '32-byte'):
            ota.verify_private_seed(base64.b64encode(b'short').decode(), wrong, self.public)

    def test_wrong_signature_is_rejected(self):
        self.feed.find('./channel/item/enclosure').set(
            '{' + ota.NAMESPACE + '}edSignature', base64.b64encode(b'\0' * 64).decode())
        self.save_feed()
        with self.assertRaisesRegex(ValueError, 'signature verification failed'):
            self.verify()

    def test_appcast_wrong_url_and_build_are_rejected(self):
        enclosure = self.feed.find('./channel/item/enclosure')
        enclosure.set('url', 'https://example.com/not-our-update.zip')
        self.save_feed()
        with self.assertRaisesRegex(ValueError, 'URL/size'):
            self.verify()
        self.feed.find('./channel/item/{' + ota.NAMESPACE + '}minimumUpdateVersion').text = '7'
        self.save_feed()
        with self.assertRaisesRegex(ValueError, 'OTA baseline'):
            self.verify()

    def test_changed_mac_archive_and_old_public_key_are_rejected(self):
        with zipfile.ZipFile(self.archive, 'a') as bundle:
            bundle.writestr('added.txt', b'tamper')
        self.feed.find('./channel/item/enclosure').set('length', str(self.archive.stat().st_size))
        self.save_feed()
        with self.assertRaisesRegex(ValueError, 'signature verification failed'):
            self.verify()
        with patch.object(ota, 'public_key', return_value=b'\0' * 32):
            with self.assertRaisesRegex(ValueError, 'different OTA public key'):
                ota.verify(self.root, self.archive, 'v0.1.5')


if __name__ == '__main__':
    unittest.main()
