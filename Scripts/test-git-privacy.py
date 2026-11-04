#!/usr/bin/env python3
"""在临时仓库验证邮箱防护，不操作已有仓库或会话。"""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


CHECK = Path(__file__).with_name('verify-git-privacy.py')
PUBLIC = '12345+contributor@users.noreply.github.com'
PRIVATE = 'developer@example.invalid'


class PrivacyTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.env = dict(os.environ, GIT_CONFIG_GLOBAL=os.devnull,
                        GIT_CONFIG_NOSYSTEM='1', GIT_AUTHOR_NAME='Contributor',
                        GIT_COMMITTER_NAME='Contributor', GIT_AUTHOR_EMAIL=PUBLIC,
                        GIT_COMMITTER_EMAIL=PUBLIC)
        self.git('init', '--initial-branch=main')
        self.tree = self.git('mktree', data='')

    def git(self, *args, data=None, **env):
        return subprocess.check_output(['git', *args], cwd=self.directory,
                                       input=data, text=True, stderr=subprocess.PIPE,
                                       env=dict(self.env, **env)).strip()

    def commit(self, parent=None, message='Fixture', **env):
        args = ['commit-tree', self.tree]
        if parent:
            args += ['-p', parent]
        oid = self.git(*args, data=message, **env)
        self.git('update-ref', 'refs/heads/main', oid)
        return oid

    def check(self, *args, data=None, **env):
        return subprocess.run([sys.executable, str(CHECK), *args],
                              cwd=self.directory, input=data, text=True,
                              capture_output=True, env=dict(self.env, **env))

    def assert_blocked(self, result):
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn(PRIVATE, result.stdout + result.stderr)

    def test_public_identity_and_history_pass(self):
        self.commit()
        self.assertEqual(self.check('--identity').returncode, 0)
        self.assertEqual(self.check().returncode, 0)

    def test_author_and_committer_override_are_blocked(self):
        for key in ('GIT_AUTHOR_EMAIL', 'GIT_COMMITTER_EMAIL'):
            with self.subTest(key=key):
                self.assert_blocked(self.check('--identity', **{key: PRIVATE}))

    def test_private_ancestor_is_blocked_before_push(self):
        old = self.commit(GIT_AUTHOR_EMAIL=PRIVATE)
        tip = self.commit(parent=old)
        self.assert_blocked(self.check('--push', data=f'refs/heads/main {tip} refs/heads/main {"0" * 40}\n'))

    def test_private_committer_in_history_is_blocked(self):
        self.commit(GIT_COMMITTER_EMAIL=PRIVATE)
        self.assert_blocked(self.check())

    def test_private_message_trailer_is_blocked(self):
        self.commit(message=f'Fixture\n\nCo-authored-by: Contributor <{PRIVATE}>\n')
        self.assert_blocked(self.check())

    def test_private_message_is_blocked_before_commit(self):
        message = self.directory / 'message'
        message.write_text(f'Fixture\n\nSigned-off-by: Contributor <{PRIVATE}>\n')
        self.assert_blocked(self.check('--message', str(message)))

    def test_private_annotated_tag_is_blocked(self):
        self.commit()
        self.git('tag', '-a', 'fixture', '-m', 'Fixture', GIT_COMMITTER_EMAIL=PRIVATE)
        self.assert_blocked(self.check())

    def test_private_tag_pushed_by_object_id_is_blocked(self):
        self.commit()
        self.git('tag', '-a', 'fixture', '-m', 'Fixture', GIT_COMMITTER_EMAIL=PRIVATE)
        tag = self.git('rev-parse', 'refs/tags/fixture')
        self.git('tag', '-d', 'fixture')
        self.assert_blocked(self.check('--push', data=f'{tag} {tag} refs/tags/fixture {"0" * 40}\n'))

    def test_deleting_a_ref_does_not_republish_it(self):
        old = self.commit(GIT_AUTHOR_EMAIL=PRIVATE)
        self.assertEqual(self.check('--push', data=f'(delete) {"0" * 40} refs/heads/main {old}\n').returncode, 0)


if __name__ == '__main__':
    unittest.main()
