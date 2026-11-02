import base64
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, os.environ.get('ANYWHERE_TEST_UI', str(ROOT / 'Linux')))
from anywhere_host.updates import verify, newer, private_copy


class SignedUpdates(unittest.TestCase):
    def test_signature_rejects_tampering_and_wrong_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            key, public, payload, signature = [root / name for name in ('key', 'public', 'payload', 'signature')]
            subprocess.run(['openssl', 'genpkey', '-algorithm', 'ED25519', '-out', str(key)], check=True)
            subprocess.run(['openssl', 'pkey', '-in', str(key), '-pubout', '-out', str(public)], check=True)
            payload.write_bytes(b'isolated package fixture')
            raw = subprocess.check_output(['openssl', 'pkeyutl', '-sign', '-inkey', str(key), '-rawin', '-in', str(payload)])
            signature.write_text(base64.b64encode(raw).decode())
            verify(payload, signature, public)
            with self.assertRaises(ValueError):
                verify(payload, signature)  # A test key is not the production release identity.
            payload.write_bytes(b'tampered package')
            with self.assertRaises(ValueError):
                verify(payload, signature, public)

    def test_private_copy_rejects_symlinks_and_oversized_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'input').write_bytes(b'fixture')
            (root / 'link').symlink_to(root / 'input')
            with self.assertRaises(OSError):
                private_copy(root / 'link', root / 'output', 100)
            with self.assertRaises(ValueError):
                private_copy(root / 'input', root / 'large', 1)
            private_copy(root / 'input', root / 'copy', 100)
            self.assertEqual((root / 'copy').read_bytes(), b'fixture')

    def test_debian_version_order_and_downgrade_rejection(self):
        self.assertTrue(newer('0.1.4-2', '0.1.4-1'))
        self.assertTrue(newer('0.1.10-1', '0.1.9-1'))
        self.assertFalse(newer('0.1.4-1', '0.1.4-1'))
        self.assertFalse(newer('0.1.3-9', '0.1.4-1'))
