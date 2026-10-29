#!/usr/bin/env python3
"""Configuration installer tests only; no Apple request or signing-key validation."""
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).with_name('configure-apns.py')


class APNsConfigTests(unittest.TestCase):
    def test_private_install_preserves_settings_and_requires_explicit_rotation(self):
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent, prefix='.apns-config-test-') as folder:
            root = pathlib.Path(folder)
            state = root / 'state'
            state.mkdir()
            config = state / 'config.json'
            config.write_text(json.dumps({'listen': '127.0.0.1:9999', 'codexHome': '/existing'}))
            original = config.read_bytes()
            key = root / 'fixture.p8'
            key.write_text('-----BEGIN PRIVATE KEY-----\nNOT-A-REAL-KEY\n-----END PRIVATE KEY-----\n')
            command = [sys.executable, str(SCRIPT), '--state-dir', str(state), '--environment', 'production',
                       '--team-id', '775Y7KV7KU', '--key-id', 'ABCDEFGHIJ', '--key-file', str(key)]
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertNotIn('NOT-A-REAL-KEY', result.stdout + result.stderr)
            saved = json.loads(config.read_text())
            self.assertEqual(config.read_bytes(), original)
            self.assertEqual(saved['listen'], '127.0.0.1:9999')
            self.assertEqual(saved['codexHome'], '/existing')
            apns_path = state / 'apns.json'
            apns = json.loads(apns_path.read_text())
            self.assertIsNone(apns['sandbox'])
            copy = pathlib.Path(apns['production']['privateKeyPath'])
            self.assertEqual(copy.read_bytes(), key.read_bytes())
            self.assertEqual(copy.stat().st_mode & 0o777, 0o600)
            self.assertEqual(apns_path.stat().st_mode & 0o777, 0o600)
            self.assertNotEqual(subprocess.run(command, capture_output=True).returncode, 0)
            self.assertEqual(subprocess.run(command + ['--replace'], capture_output=True).returncode, 0)
            # Existing inline configurations remain authoritative and retain
            # both unrelated settings and the other APNs environment.
            saved['apns'] = apns
            config.write_text(json.dumps(saved))
            sandbox = command.copy()
            sandbox[sandbox.index('production')] = 'sandbox'
            self.assertEqual(subprocess.run(sandbox, capture_output=True).returncode, 0)
            updated = json.loads(config.read_text())
            self.assertEqual(updated['listen'], saved['listen'])
            self.assertEqual(updated['apns']['production'], apns['production'])
            self.assertIsNotNone(updated['apns']['sandbox'])


if __name__ == '__main__':
    unittest.main()
