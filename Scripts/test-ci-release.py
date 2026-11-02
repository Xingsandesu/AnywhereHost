#!/usr/bin/env python3
"""Reject untrusted build provenance and tampered/ambiguous release artifacts."""
import copy
import hashlib
import importlib.util
import pathlib
import tempfile
import unittest

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


if __name__ == '__main__':
    unittest.main()
