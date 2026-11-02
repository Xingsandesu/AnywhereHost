"""Local password persistence without enabling sharing or touching user data."""
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, os.environ.get('ANYWHERE_TEST_UI', str(ROOT / 'Linux')))
from anywhere_host.service import HostService


class DesktopPassword(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='anywhere-password-test-')
        self.addCleanup(temporary.cleanup)
        self.service = HostService(os.environ.get('ANYWHERE_TEST_BINARY', ROOT / 'target/debug/anywhere-host'),
                                   Path(temporary.name) / 'state')
        self.password = self.service.directory / 'desktop.passwd'
        self.settings = self.service.directory / 'desktop.json'

    def test_save_offline_and_change_while_enabled_preserve_sharing_state(self):
        self.assertFalse(self.service.has_desktop_password())
        self.service.save_desktop_password('test1234')
        original = self.password.read_bytes()
        self.assertEqual(len(original), 8)
        self.assertNotEqual(original, b'test1234')
        self.assertEqual(self.password.stat().st_mode & 0o777, 0o600)
        self.assertFalse(self.settings.exists())
        self.assertTrue(HostService(self.service.binary, self.service.directory).has_desktop_password())
        self.service.set_desktop(True)
        self.service.save_desktop_password('new12345')
        self.assertNotEqual(self.password.read_bytes(), original)
        self.assertEqual(json.loads(self.settings.read_text()), {'enabled': True})
        self.service.set_desktop(False)
        self.assertTrue(self.service.has_desktop_password())

    def test_invalid_password_does_not_replace_saved_credentials(self):
        self.service.set_desktop(True, 'test1234')
        original = self.password.read_bytes()
        for value in ('', 'short', 'toolong12', '中文密码测试八位', 'test123\n', 'test123\x7f'):
            with self.subTest(value=value):
                with self.assertRaisesRegex(ValueError, '8 位'):
                    self.service.save_desktop_password(value)
                self.assertEqual(self.password.read_bytes(), original)
                self.assertEqual(json.loads(self.settings.read_text()), {'enabled': True})

    def test_missing_helper_keeps_saved_password_and_reports_recovery(self):
        self.service.save_desktop_password('test1234')
        original = self.password.read_bytes()
        missing = HostService(self.service.directory / 'missing/anywhere-host', self.service.directory)
        with self.assertRaisesRegex(RuntimeError, 'vncpasswd'):
            missing.save_desktop_password('new12345')
        self.assertEqual(self.password.read_bytes(), original)
        self.assertFalse(self.settings.exists())

    def test_enable_requires_valid_private_password_file(self):
        with self.assertRaises(ValueError):
            self.service.set_desktop(True)
        self.service.save_desktop_password('test1234')
        self.password.chmod(0o644)
        self.assertFalse(self.service.has_desktop_password())
        with self.assertRaises(ValueError):
            self.service.set_desktop(True)
        self.assertFalse(self.settings.exists())
        self.service.save_desktop_password('new12345')
        self.service.set_desktop(True)
        self.assertTrue(self.service.has_desktop_password())


if __name__ == '__main__':
    unittest.main()
