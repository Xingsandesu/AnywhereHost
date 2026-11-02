import json
import os
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, os.environ.get('ANYWHERE_TEST_UI', str(ROOT / 'Linux')))
from anywhere_host import updates


class UpdateArchitecture(unittest.TestCase):
    def test_selects_only_signed_native_asset_from_mixed_release(self):
        assets = []
        for architecture in ('amd64', 'arm64'):
            name = f'anywhere-host_0.1.5-1_{architecture}.deb'
            for suffix in ('', '.sig'):
                assets.append({'name': name + suffix, 'browser_download_url':
                               updates.REPOSITORY + '/releases/download/v0.1.5/' + name + suffix})

        def download_release(url, path, limit):
            path.write_text(json.dumps({'assets': assets}))

        for architecture in ('amd64', 'arm64'):
            with self.subTest(architecture=architecture), \
                    patch.object(updates, 'native_architecture', return_value=architecture), \
                    patch.object(updates, 'download', side_effect=download_release), \
                    patch.object(updates.Updater, 'current_version', return_value='0.1.4-1'):
                updater = updates.Updater(None)
                updater.check()
                self.assertEqual(updater.candidate['name'], f'anywhere-host_0.1.5-1_{architecture}.deb')
                native_signature = next(asset for asset in assets
                                        if asset['name'] == updater.candidate['name'] + '.sig')
                assets.remove(native_signature)
                self.assertIn(f'Debian {architecture}', updater.check())
                self.assertIsNone(updater.candidate)
                assets.append(native_signature)

    def test_fetch_rejects_signed_package_with_wrong_architecture(self):
        for native, foreign in (('arm64', 'amd64'), ('amd64', 'arm64')):
            with self.subTest(architecture=native), \
                    patch.object(updates, 'native_architecture', return_value=native), \
                    patch.object(updates, 'download'), patch.object(updates, 'verify'), \
                    patch.object(updates, 'metadata', return_value={
                        'Package': 'anywhere-host', 'Version': '0.1.5-1', 'Architecture': foreign}):
                updater = updates.Updater(None)
                updater.candidate = {'name': f'anywhere-host_0.1.5-1_{native}.deb',
                                     'version': '0.1.5-1', 'url': 'fixture', 'signature': 'fixture'}
                try:
                    with self.assertRaisesRegex(ValueError, '身份'):
                        updater.fetch()
                    self.assertIsNone(updater.downloaded)
                finally:
                    updater.close()

    def test_privileged_installer_rejects_foreign_architecture_before_apt(self):
        for native, foreign in (('arm64', 'amd64'), ('amd64', 'arm64')):
            with self.subTest(architecture=native), \
                    patch.object(updates.os, 'getuid', return_value=0), \
                    patch.object(updates, 'native_architecture', return_value=native), \
                    patch.object(updates, 'private_copy'), patch.object(updates, 'verify'), \
                    patch.object(updates, 'metadata', return_value={
                        'Package': 'anywhere-host', 'Version': '0.1.5-1', 'Architecture': foreign}), \
                    patch.object(updates.subprocess, 'check_output', return_value='0.1.4-1'), \
                    patch.object(updates.subprocess, 'run') as command:
                with self.assertRaisesRegex(ValueError, '身份'):
                    updates.install_verified('fixture.deb', 'fixture.deb.sig')
                command.assert_not_called()

    def test_unsupported_debian_architecture_fails_explicitly(self):
        with patch.object(updates.subprocess, 'check_output', return_value='riscv64\n'):
            with self.assertRaisesRegex(ValueError, 'riscv64'):
                updates.native_architecture()
