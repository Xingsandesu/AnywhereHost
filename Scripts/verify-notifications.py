#!/usr/bin/env python3
"""显式检查线上 relay-v2 的拒绝边界；不投递 APNs、不读取用户会话。"""
import argparse
import json
import subprocess

ORIGIN = 'https://connect.jintongshu.com'

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--remote', action='store_true', required=True)
    parser.parse_args()
    def call(path, value, authorization=None):
        headers = ['Content-Type: application/json']
        if authorization:
            headers.append('Authorization: ' + authorization)
        config = 'silent\nshow-error\nmax-time = 30\nurl = ' + json.dumps(ORIGIN + path) + '\n'
        config += ''.join('header = ' + json.dumps(h) + '\n' for h in headers)
        config += 'data = ' + json.dumps(json.dumps(value)) + '\nwrite-out = "\\n%{http_code}"\n'
        result = subprocess.run(['curl', '--config', '-'], input=config, capture_output=True, text=True, check=True)
        raw, status = result.stdout.rsplit('\n', 1)
        return int(status), json.loads(raw)
    for path in ['/v1/status', '/v1/grants', '/v1/registrations', '/v1/validate', '/v1/deliver']:
        assert call(path, {})[0] == 410
    status, value = call('/v2/status', {})
    assert status == 200 and value['transport'] == 'relay-v2' and value['environments'] == ['production'] and value['enabled']
    assert call('/v2/deliver', {}, 'Bearer ' + '0' * 64)[0] == 403
    assert call('/v2/deliver', {'deliveryToken': '0' * 128, 'payload': {}})[0] == 410
    assert call('/v2/subscriptions', {'keyId': 'unknown', 'challenge': '0' * 64, 'data': 'e30=', 'assertion': 'e30='})[0] == 401
    print('PASS relay-v2 production configuration, retired endpoints, bearer rejection, unknown client and subscription rejection')
    print('App Attest enrollment and lock-screen display require the released client on a physical device.')

if __name__ == '__main__':
    main()
