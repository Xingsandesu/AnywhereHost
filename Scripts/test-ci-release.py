#!/usr/bin/env python3
"""Reject untrusted build provenance and tampered/ambiguous release artifacts."""
import copy
import hashlib
import importlib.util
import io
import json
import os
import pathlib
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('ci_release', pathlib.Path(__file__).with_name('ci-release.py'))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class SourceTests(unittest.TestCase):
    def setUp(self):
        self.repo = 'Xingsandesu/AnywhereHost'
        self.run = {
            'repository': {'full_name': self.repo},
            'head_repository': {'full_name': self.repo},
            'path': '.github/workflows/build.yml', 'event': 'push',
            'head_branch': 'main', 'status': 'completed', 'conclusion': 'success',
            'head_sha': 'a' * 40,
        }

    def test_successful_main_push_and_manual_build(self):
        for event in ('push', 'workflow_dispatch'):
            self.run['event'] = event
            self.assertEqual(release.validate_run(self.run, self.repo), 'a' * 40)

    def test_pr_fork_branch_failure_and_wrong_workflow_are_rejected(self):
        changes = {
            'event': 'pull_request', 'head_branch': 'feature',
            'status': 'in_progress', 'conclusion': 'failure',
            'path': '.github/workflows/untrusted.yml',
            'head_sha': 'main\ninjected=value',
            'repository': {'full_name': 'attacker/AnywhereHost'},
            'head_repository': {'full_name': 'attacker/AnywhereHost'},
        }
        for field, value in changes.items():
            with self.subTest(field=field):
                run = copy.deepcopy(self.run)
                run[field] = value
                with self.assertRaises(ValueError):
                    release.validate_run(run, self.repo)


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = pathlib.Path(self.temporary.name)
        self.package = self.directory / 'anywhere-host_0.1.4-3_arm64.deb'
        self.package.write_bytes(b'fixture package bytes')
        self.checksum = hashlib.sha256(self.package.read_bytes()).hexdigest()
        self.manifest = self.directory / 'SHA256SUMS'
        self.manifest.write_text(f'{self.checksum}  {self.package.name}\n')

    def test_checksum_and_platform(self):
        self.assertEqual(release.package(self.directory, 'arm64'), self.package)
        self.manifest.write_text(f'{self.checksum}  ./{self.package.name}\n')
        self.assertEqual(release.package(self.directory, 'arm64'), self.package)
        with self.assertRaises(ValueError):
            release.package(self.directory, 'amd64')

    def test_changed_package_is_rejected(self):
        self.package.write_bytes(b'tampered')
        with self.assertRaises(ValueError):
            release.package(self.directory, 'arm64')

    def test_extra_package_is_rejected(self):
        (self.directory / 'anywhere-host_0.1.4-2_arm64.deb').write_bytes(b'old')
        with self.assertRaises(ValueError):
            release.package(self.directory, 'arm64')

    def test_manifest_path_and_multiple_entries_are_rejected(self):
        for filename in ('../' + self.package.name, 'nested/' + self.package.name,
                         'wrong.deb', self.package.name + '\n' + self.checksum + '  second.deb'):
            with self.subTest(filename=filename):
                self.manifest.write_text(f'{self.checksum}  {filename}\n')
                with self.assertRaises(ValueError):
                    release.package(self.directory, 'arm64')

    def test_symlink_package_or_manifest_is_rejected(self):
        with tempfile.TemporaryDirectory() as other:
            target = pathlib.Path(other) / 'original'
            self.package.replace(target)
            self.package.symlink_to(target)
            with self.assertRaises(ValueError):
                release.package(self.directory, 'arm64')
            self.package.unlink()
            target.replace(self.package)
            self.manifest.replace(target)
            self.manifest.symlink_to(target)
            with self.assertRaises(ValueError):
                release.package(self.directory, 'arm64')


class PublicationTests(unittest.TestCase):
    def test_downloads_use_published_urls_not_draft_urls(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = pathlib.Path(temporary)
            contents = {'AnywhereHost-0.1.4-macos-arm64-notarized.zip': b'archive',
                        'SHA256SUMS': b'manifest'}
            for name, data in contents.items():
                (directory / name).write_bytes(data)
            (directory / 'notes.md').write_text('fixture')
            sha = 'a' * 40
            tag = 'v0.1.4-signed.' + sha[:12]
            draft = {
                'draft': True, 'prerelease': True, 'target_commitish': sha,
                'tag_name': tag, 'html_url': 'https://github.com/example/repo/releases/tag/untagged-draft',
                'assets': [{'name': name, 'digest': 'sha256:' + hashlib.sha256(data).hexdigest(),
                            'browser_download_url': 'https://github.com/example/repo/releases/download/untagged-draft/' + name}
                           for name, data in contents.items()],
            }
            published = copy.deepcopy(draft)
            published['draft'] = False
            published['html_url'] = published['html_url'].replace('untagged-draft', tag)
            for asset in published['assets']:
                asset['browser_download_url'] = asset['browser_download_url'].replace('untagged-draft', tag)

            def download(url, **kwargs):
                self.assertNotIn('untagged-draft', url)
                self.assertIn('/' + tag + '/', url)
                return io.BytesIO(contents[url.rsplit('/', 1)[1]])

            with patch.dict(os.environ, GITHUB_REPOSITORY='example/repo'), \
                    patch.object(release.subprocess, 'run'), \
                    patch.object(release.subprocess, 'check_output', return_value=json.dumps({'databaseId': 42})), \
                    patch.object(release, 'gh_api', side_effect=[draft, published]), \
                    patch.object(release.urllib.request, 'urlopen', side_effect=download) as fetch, \
                    patch('sys.stdout', new_callable=io.StringIO):
                release.publish(directory, tag, sha)
                self.assertEqual(fetch.call_count, len(contents))


if __name__ == '__main__':
    unittest.main()
