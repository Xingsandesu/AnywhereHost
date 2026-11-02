"""Disposable Rust peer tests; never read or mutate the user's Codex threads."""
import json
import os
from pathlib import Path
import socket
import ssl
import sys
import tempfile
import time
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, os.environ.get('ANYWHERE_TEST_UI', str(ROOT / 'Linux')))
sys.path.insert(0, str(ROOT / 'Scripts'))
from anywhere_host.client import HostClient
from anywhere_host.paths import state_directory
from anywhere_host.service import HostService, private_json
from anywhere_host.preferences import PermissionChecks
from security_wire import Device, admin_headers


class PathsAndSettings(unittest.TestCase):
    def test_cancelled_recheck_clears_previous_success(self):
        checks = PermissionChecks(None, None)
        checks.results = [('fixture', '/fixture', True, {'readable': True, 'writable': True})]
        self.assertTrue(checks.ready)

        def cancelled_targets():
            checks.cancelled.set()
            return [('fixture', '/fixture', True)]

        checks.targets = cancelled_targets
        self.assertEqual(checks.run(), [])
        self.assertFalse(checks.ready)

    def test_xdg_paths_and_relative_fallback(self):
        for xdg, expected in [('/custom/state', '/custom/state/AnywhereHost'),
                              ('relative', '/example/.local/state/AnywhereHost'),
                              ('', '/example/.local/state/AnywhereHost')]:
            with patch.dict(os.environ, HOME='/example', XDG_STATE_HOME=xdg):
                self.assertEqual(str(state_directory()), expected)

    def test_private_settings_preserve_other_backend_configuration(self):
        with tempfile.TemporaryDirectory() as temporary:
            service = HostService(ROOT / 'target/debug/anywhere-host', temporary)
            private_json(service.directory / 'config.json', {'apns': {'topic': 'retained'}})
            service.save_settings('/custom/codex', '/custom/bin/codex', '/custom/control.sock', '9445')
            self.assertEqual(service.settings()['apns'], {'topic': 'retained'})
            self.assertEqual((service.directory / 'config.json').stat().st_mode & 0o777, 0o600)
            with self.assertRaises(ValueError):
                service.save_settings('relative', '', '', '9445')


class ManagementIntegration(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory(prefix='anywhere-linux-test-')
        cls.directory = Path(cls.temporary.name)
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            cls.port = listener.getsockname()[1]
        private_json(cls.directory / 'config.json', {
            'listen': f'127.0.0.1:{cls.port}', 'codexHome': str(cls.directory),
            'codexBinary': str(ROOT / 'Scripts/fake-codex.py'),
        })
        cls.service = HostService(os.environ.get('ANYWHERE_TEST_BINARY', ROOT / 'target/debug/anywhere-host'), cls.directory)
        cls.service.start()
        try:
            for _ in range(200):
                try:
                    client = HostClient(cls.directory)
                    if client.request('/v1/status')['connected']:
                        break
                except (OSError, ValueError):
                    pass
                time.sleep(.05)
            else:
                raise AssertionError(cls.service.log_tail())
        except BaseException:
            cls.service.stop()
            cls.temporary.cleanup()
            raise

    @classmethod
    def tearDownClass(cls):
        cls.service.stop()
        cls.temporary.cleanup()

    def test_signed_management_and_linux_capabilities(self):
        client = HostClient(self.directory)
        self.assertTrue(client.request('/v1/status')['connected'])
        supported = client.request('/v1/capabilities')['remoteDesktop']['supported']
        private_json(self.directory / 'desktop.json', {'enabled': True})
        desktop = client.request('/v1/desktop')
        self.assertEqual(desktop['supported'], supported)
        self.assertFalse(desktop['reachable'])  # No local password or sharing consent was provided.
        headers = admin_headers(self.port, client.key.decode(), client.certificate, 'GET', '/v1/desktop/socket')
        headers.update({'Connection': 'Upgrade', 'Upgrade': 'websocket',
                        'Sec-WebSocket-Version': '13', 'Sec-WebSocket-Key': 'MDEyMzQ1Njc4OWFiY2RlZg=='})
        with self.assertRaises(RuntimeError):
            client._request('/v1/desktop/socket', headers=headers)
        private_json(self.directory / 'desktop.json', {'enabled': False})

    def test_certificate_mismatch_and_wrong_admin_key_fail_closed(self):
        client = HostClient(self.directory)
        client.certificate = b'untrusted peer'
        with self.assertRaises(ssl.SSLError):
            client.request('/v1/status')
        client = HostClient(self.directory)
        client.key = b'wrong key'
        with self.assertRaisesRegex(RuntimeError, 'invalid_proof'):
            client.request('/v1/status')

    def test_pairing_enrollment_and_revocation(self):
        client = HostClient(self.directory)
        for endpoint in ('http://example.test', 'https://u:p@example.test', 'https://example.test/path',
                         'https://example.test/?secret=1', 'https://example.test:99999'):
            with self.assertRaises(ValueError):
                client.pairing_link(endpoint)
        link, expires = client.pairing_link('https://example.test:9443')
        values = parse_qs(urlsplit(link).query)
        self.assertGreater(expires, time.time())
        self.assertEqual(values['certificateSHA256'], [client.fingerprint])
        device = Device(self.directory)
        headers = device.headers(self.port, client.certificate, 'POST', '/v1/security/pair', '{}',
                                 ticket=values['ticket'][0])
        enrolled = client._request('/v1/security/pair', b'{}', headers)
        self.assertEqual(enrolled['deviceID'], device.id)
        self.assertIn(device.id, client.request('/v1/security/devices')['devices'])
        client.request('/v1/security/devices/revoke', {'deviceID': device.id})
        self.assertNotIn(device.id, client.request('/v1/security/devices')['devices'])

    def test_duplicate_child_never_stops_external_owner(self):
        duplicate = HostService(self.service.binary, self.directory)
        duplicate.start()
        self.assertNotEqual(duplicate.child.wait(timeout=10), 0)
        duplicate.stop()
        with self.assertRaisesRegex(RuntimeError, '外部 Host'):
            duplicate.save_settings(str(self.directory), '', '', self.port)
        self.assertTrue(self.service.running)
        self.assertTrue(HostClient(self.directory).request('/v1/status')['connected'])
        with self.assertRaises(RuntimeError):
            self.service.save_settings(str(self.directory), '', '', self.port)
        result = self.service.check_directory(str(self.directory))
        self.assertTrue(result['readable'] and result['writable'])


if __name__ == '__main__':
    unittest.main()
