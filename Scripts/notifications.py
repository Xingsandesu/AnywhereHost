#!/usr/bin/env python3
"""管理统一通知服务；凭据只经 stdin、受保护文件和 HTTPS 传输。"""
import argparse
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
WORKER = ROOT / 'notification-worker'


def wrangler(*args, data=None):
    subprocess.run(['node', str(WORKER / 'node_modules/wrangler/bin/wrangler.js'), *args],
                   cwd=WORKER, input=data, text=True, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    upload = sub.add_parser('upload-apns')
    upload.add_argument('--team-id', required=True)
    upload.add_argument('--production-key-id', required=True)
    upload.add_argument('--production-key', type=Path, required=True)
    args = parser.parse_args()
    if args.team_id != '775Y7KV7KU':
        parser.error('Unexpected production team')
    path = args.production_key
    if not re.fullmatch(r'[A-Z0-9]{10}', args.production_key_id) or path.is_symlink() or not path.is_file() or path.stat().st_mode & 0o077 or path.stat().st_size > 16384:
        parser.error('Invalid key or private file permissions (require chmod 600)')
    private_key = path.read_text()
    checked = subprocess.run(['openssl', 'pkey', '-check', '-noout'], input=private_key,
                             text=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    if checked.returncode or not private_key.startswith('-----BEGIN PRIVATE KEY-----'):
        parser.error('Invalid PKCS#8 key')
    config = {'production': {'teamId': args.team_id, 'keyId': args.production_key_id, 'privateKey': private_key}}
    wrangler('secret', 'put', 'APNS_CONFIG', data=json.dumps(config))


if __name__ == '__main__':
    main()
