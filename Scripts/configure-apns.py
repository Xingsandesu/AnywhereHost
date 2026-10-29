#!/usr/bin/env python3
"""Install a local APNs signing key/config without printing or transmitting it."""
import argparse
import json
import os
import pathlib
import re
import tempfile


def private_write(path, data):
    descriptor, temporary = tempfile.mkstemp(prefix='.apns-', dir=path.parent)
    try:
        with os.fdopen(descriptor, 'wb') as output:
            os.fchmod(output.fileno(), 0o600)
            output.write(data)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--state-dir', required=True, type=pathlib.Path)
    parser.add_argument('--environment', required=True, choices=('sandbox', 'production'))
    parser.add_argument('--team-id', required=True)
    parser.add_argument('--key-id', required=True)
    parser.add_argument('--key-file', required=True, type=pathlib.Path)
    parser.add_argument('--replace', action='store_true', help='Explicitly replace this environment’s existing signing key')
    args = parser.parse_args()
    for value in (args.team_id, args.key_id):
        if not re.fullmatch(r'[A-Z0-9]{10}', value):
            parser.error('Team ID and Key ID must contain 10 uppercase letters/digits')
    state = args.state_dir.expanduser().resolve()
    config_path = state / 'config.json'
    if not config_path.is_file():
        parser.error('Use the existing Host state directory containing config.json')
    config = json.loads(config_path.read_text())
    # Old Host versions reject unknown config.json fields. A separate file is
    # safely ignored until a new binary with APNs support is installed.
    inline = bool(config.get('apns'))
    apns_path = state / 'apns.json'
    existing = config['apns'] if inline else (json.loads(apns_path.read_text()) if apns_path.exists() else None)
    apns = existing or {'teamId': args.team_id, 'bundleId': 'com.jintongshu.AnyWhereCoding', 'sandbox': None, 'production': None}
    if apns['teamId'] != args.team_id:
        parser.error('The existing APNs configuration uses another team')
    if apns.get('bundleId') != 'com.jintongshu.AnyWhereCoding':
        parser.error('The existing APNs configuration uses another application')
    if apns.get(args.environment) and not args.replace:
        parser.error('This environment is configured; use --replace for an intentional key rotation')
    key = args.key_file.expanduser().read_bytes()
    if len(key) > 16384 or b'-----BEGIN PRIVATE KEY-----' not in key:
        parser.error('Expected an APNs PKCS#8 .p8 private key file')
    # Copy into the Host state directory. The key contents
    # never appear in process arguments, config.json, logs, or API responses.
    destination = state / ('apns-' + args.environment + '-' + args.key_id + '.p8')
    if destination.exists() and not args.replace:
        parser.error('A signing key file already exists; use --replace to rotate it')
    private_write(destination, key)
    apns[args.environment] = {'keyId': args.key_id, 'privateKeyPath': str(destination)}
    if inline:
        config['apns'] = apns
        private_write(config_path, (json.dumps(config, indent=2) + '\n').encode())
    else:
        private_write(apns_path, (json.dumps(apns, indent=2) + '\n').encode())
    print('APNs configuration saved for ' + args.environment + '. A Host build with APNs support loads it at startup.')


if __name__ == '__main__':
    main()
