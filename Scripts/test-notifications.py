#!/usr/bin/env python3
"""生产通知密钥工具检查；不连接云账户。"""
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch
spec = importlib.util.spec_from_file_location('notifications', Path(__file__).with_name('notifications.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class ToolTests(unittest.TestCase):
    def test_legacy_enrollment_and_ticket_commands_are_removed(self):
        for command in ['enroll', 'revoke', 'create-ticket-key']:
            with self.subTest(command=command), patch('sys.argv', ['notifications.py', command]), patch.object(module, 'wrangler') as cloud:
                with self.assertRaises(SystemExit): module.main()
                cloud.assert_not_called()
    def test_wrong_team_never_updates_cloud_secret(self):
        with patch('sys.argv', ['notifications.py','upload-apns','--team-id','WRONGTEAM1','--production-key-id','ABCDEFGHIJ','--production-key','/not/a/key']), patch.object(module,'wrangler') as cloud:
            with self.assertRaises(SystemExit): module.main()
            cloud.assert_not_called()
if __name__ == '__main__': unittest.main()
