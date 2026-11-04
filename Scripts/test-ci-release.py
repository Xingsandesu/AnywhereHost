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

    def test_stable_release_requires_current_main_and_workflow_commit(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = pathlib.Path(temporary) / 'output'
            for comparison, workflow_sha, accepted in (
                ('identical', 'a' * 40, True),
                ('ahead', 'a' * 40, False),
                ('identical', 'b' * 40, False),
            ):
                with self.subTest(comparison=comparison, workflow_sha=workflow_sha), \
                        patch.dict(os.environ, GITHUB_REF='refs/heads/main', GITHUB_REPOSITORY=self.repo,
                                   GITHUB_SHA=workflow_sha, GITHUB_OUTPUT=str(output)), \
                        patch.object(release, 'gh_api', side_effect=[self.run, {'status': comparison}]), \
                        patch('sys.stdout', new_callable=io.StringIO):
                    if accepted:
                        release.source('123', stable=True)
                        self.assertIn('sha=' + 'a' * 40, output.read_text())
                    else:
                        with self.assertRaisesRegex(ValueError, 'current main commit'):
                            release.source('123', stable=True)


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = pathlib.Path(self.temporary.name)
        self.package = self.directory / 'AnywhereHost-0.1.4-macos-arm64-notarized.zip'
        self.package.write_bytes(b'fixture package bytes')
        self.checksum = hashlib.sha256(self.package.read_bytes()).hexdigest()
        self.manifest = self.directory / 'SHA256SUMS'
        self.manifest.write_text(f'{self.checksum}  {self.package.name}\n')

    def test_checksum_and_signing_kind(self):
        self.assertEqual(release.package(self.directory, 'notarized'), self.package)
        self.manifest.write_text(f'{self.checksum}  ./{self.package.name}\n')
        self.assertEqual(release.package(self.directory, 'notarized'), self.package)
        with self.assertRaises(ValueError):
            release.package(self.directory, 'unsigned')

    def test_changed_package_is_rejected(self):
        self.package.write_bytes(b'tampered')
        with self.assertRaises(ValueError):
            release.package(self.directory, 'notarized')

    def test_extra_package_is_rejected(self):
        (self.directory / 'AnywhereHost-0.1.3-macos-arm64-notarized.zip').write_bytes(b'old')
        with self.assertRaises(ValueError):
            release.package(self.directory, 'notarized')

    def test_manifest_path_and_multiple_entries_are_rejected(self):
        for filename in ('../' + self.package.name, 'nested/' + self.package.name,
                         'wrong.zip', self.package.name + '\n' + self.checksum + '  second.zip'):
            with self.subTest(filename=filename):
                self.manifest.write_text(f'{self.checksum}  {filename}\n')
                with self.assertRaises(ValueError):
                    release.package(self.directory, 'notarized')

    def test_symlink_package_or_manifest_is_rejected(self):
        with tempfile.TemporaryDirectory() as other:
            target = pathlib.Path(other) / 'original'
            self.package.replace(target)
            self.package.symlink_to(target)
            with self.assertRaises(ValueError):
                release.package(self.directory, 'notarized')
            self.package.unlink()
            target.replace(self.package)
            self.manifest.replace(target)
            self.manifest.symlink_to(target)
            with self.assertRaises(ValueError):
                release.package(self.directory, 'notarized')


class ReleaseNotesTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.directory = pathlib.Path(temporary.name)
        self.changelog = self.directory / 'CHANGELOG.md'
        root = patch.object(release.ota, 'ROOT', self.directory)
        root.start()
        self.addCleanup(root.stop)

    def test_exact_version_excludes_adjacent_releases(self):
        self.changelog.write_text(
            '# 更新记录\n\n## 0.1.80 (20)\n\n- 后续版本。\n\n'
            '## 0.1.8 (11)\n\n- Current behavior. / 当前行为。\n\n'
            '## 0.1.7 (10)\n\n- 旧版行为。\n')
        self.assertEqual(release.release_notes('0.1.8'),
                         '## 0.1.8 (11)\n\n- Current behavior. / 当前行为。\n')

    def test_final_version_without_build_number(self):
        self.changelog.write_text('# 更新记录\n\n## 0.1.8\n\n- 当前行为。\n')
        self.assertEqual(release.release_notes('0.1.8'), '## 0.1.8\n\n- 当前行为。\n')

    def test_missing_or_empty_version_is_rejected(self):
        for text in ('## 0.1.80 (20)\n\n- 后续版本。\n',
                     '## 0.1.8 (11)\n\n## 0.1.7 (10)\n\n- 旧版行为。\n'):
            with self.subTest(text=text):
                self.changelog.write_text(text)
                with self.assertRaisesRegex(ValueError, 'Missing changelog entry'):
                    release.release_notes('0.1.8')


class PublicationTests(unittest.TestCase):
    def test_stable_publishes_verified_app_and_feed(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = pathlib.Path(temporary)
            contents = {'AnywhereHost-1.1.2-macos-arm64-notarized.zip': b'archive',
                        'appcast.xml': b'feed', 'SHA256SUMS': b'manifest'}
            for name, data in contents.items():
                (directory / name).write_bytes(data)
            (directory / 'notes.md').write_text('fixture')
            sha, tag = 'a' * 40, 'v1.1.2'
            draft = {
                'id': 42, 'draft': True, 'prerelease': False, 'target_commitish': sha,
                'tag_name': tag, 'html_url': 'https://github.com/example/repo/releases/tag/' + tag,
                'assets': [{'name': name, 'digest': 'sha256:' + hashlib.sha256(data).hexdigest(),
                            'browser_download_url': 'https://github.com/example/repo/releases/download/' + tag + '/' + name}
                           for name, data in contents.items()],
            }
            published = copy.deepcopy(draft)
            published['draft'] = False
            latest = {'tag_name': 'v1.1.1', 'assets': [
                {'name': 'appcast.xml', 'browser_download_url': 'https://example.com/previous.xml'}]}
            previous = ('<rss xmlns:sparkle="' + release.ota.NAMESPACE + '"><channel><item>'
                        '<sparkle:version>51</sparkle:version></item></channel></rss>').encode()

            def download(url, **kwargs):
                return io.BytesIO(previous if url.endswith('previous.xml') else contents[url.rsplit('/', 1)[1]])

            with patch.dict(os.environ, GITHUB_REPOSITORY='example/repo'), \
                    patch.object(release.subprocess, 'run') as run, \
                    patch.object(release.subprocess, 'check_output', return_value=json.dumps({'databaseId': 42})), \
                    patch.object(release, 'gh_api', side_effect=[{'object': {'sha': sha}}, latest, draft, published, {'id': 42}]), \
                    patch.object(release.urllib.request, 'urlopen', side_effect=download), \
                    patch.object(release.ota, 'app_info', return_value={'CFBundleVersion': '52'}), \
                    patch.object(release.ota, 'verify') as verify, \
                    patch('sys.stdout', new_callable=io.StringIO):
                release.publish(directory, tag, sha, stable=True)
                self.assertEqual(verify.call_count, 2)
                self.assertTrue(all(call.args[2] == tag for call in verify.call_args_list))
                commands = [call.args[0] for call in run.call_args_list]
                self.assertEqual(len(commands), 2)
                self.assertIn('--latest=true', commands[1])
                self.assertNotIn('--prerelease', commands[0])

    def test_stable_requires_exactly_one_archive(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = pathlib.Path(temporary)
            with patch.dict(os.environ, GITHUB_REPOSITORY='example/repo'):
                for names in ([], ['first.zip', 'second.zip']):
                    for name in names:
                        (directory / name).write_bytes(b'archive')
                    with self.assertRaisesRegex(ValueError, 'exactly one app archive'):
                        release.publish(directory, 'v1.1.2', 'a' * 40, stable=True)

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


class AssemblyTests(unittest.TestCase):
    def test_stable_requires_verified_ota_metadata(self):
        import zipfile, plistlib
        with tempfile.TemporaryDirectory() as temporary:
            root=pathlib.Path(temporary); artifacts=root/'artifacts'; mac=artifacts/'notarized';mac.mkdir(parents=True)
            archive=mac/'AnywhereHost-1.1.1-macos-arm64-notarized.zip'
            with zipfile.ZipFile(archive,'w') as z:
                z.writestr('AnywhereHost.app/Contents/Info.plist',plistlib.dumps({'CFBundleShortVersionString':'1.1.1','CFBundleVersion':'51'}))
            (mac/'SHA256SUMS').write_text(f'{release.digest(archive)}  {archive.name}\n')
            with patch.dict(os.environ,GITHUB_OUTPUT=str(root/'output'),GITHUB_REPOSITORY='example/repo'):
                release.assemble(artifacts,root/'release','a'*40,'123')
                self.assertEqual(len(list((root/'release').glob('*.zip'))),1)
                metadata = artifacts / 'ota'
                metadata.mkdir()
                with self.assertRaisesRegex(ValueError, 'OTA metadata'):
                    release.assemble(artifacts, root/'stable', 'a'*40, '123', stable=True)
                (metadata/'appcast.xml').write_text('fixture')
                with patch.object(release.ota, 'verify', side_effect=ValueError('invalid signature')):
                    with self.assertRaisesRegex(ValueError, 'invalid signature'):
                        release.assemble(artifacts, root/'stable', 'a'*40, '123', stable=True)
                self.assertFalse((root/'stable').exists())
                with patch.object(release.ota, 'verify') as verify:
                    release.assemble(artifacts, root/'stable', 'a'*40, '123', stable=True)
                    verify.assert_called_once_with(metadata, archive, 'v1.1.1')
                self.assertEqual({path.name for path in (root/'stable').iterdir()},
                                 {archive.name, 'appcast.xml', 'SHA256SUMS', 'notes.md'})
                self.assertIn('Sparkle Ed25519', (root/'stable/notes.md').read_text())
                self.assertIn('tag=v1.1.1\n', (root/'output').read_text())


if __name__ == '__main__':
    unittest.main()
