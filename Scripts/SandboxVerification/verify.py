#!/usr/bin/env python3
"""Verify the sandboxed app harness, never an unsandboxed backend substitute.

The harness must be running and have a real Powerbox grant. Inference is opt-in
and only creates an ephemeral test-owned thread and disposable workspace.
"""
import argparse
import http.client
import json
import pathlib
import socket
import ssl
import subprocess
import sys
import tarfile
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'Scripts'))
from host_wire import WebSocket

parser = argparse.ArgumentParser()
parser.add_argument('--inference', action='store_true')

args = parser.parse_args()
directory = pathlib.Path.home() / 'Library/Containers/com.jintongshu.AnywhereHost.SandboxVerification/Data/Library/Application Support/SandboxVerification'
port = 19443
certificate = ssl.PEM_cert_to_DER_cert((directory / 'host-cert.pem').read_text())
token = (directory / 'access-token').read_text()

def request(path, body=None, listen=port):
    conn = http.client.HTTPSConnection('127.0.0.1', listen, context=ssl._create_unverified_context(), timeout=70)
    conn.connect()
    assert conn.sock.getpeercert(binary_form=True) == certificate
    conn.request('GET' if body is None else 'POST', path, None if body is None else json.dumps(body),
                 {'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'})
    response = conn.getresponse()
    result = json.loads(response.read())
    conn.close()
    assert response.status == 200, result
    return result

def rpc(method, params):
    value = request('/v1/rpc', {'method': method, 'params': params})
    assert 'error' not in value, value
    return value['result']

checks = json.loads((directory / 'verification-checks.json').read_text())
assert checks and all(checks.values()), checks
print('PASS home bookmark restored; parent and spawned child denied ungranted external directory', flush=True)
for _ in range(120):
    status = request('/v1/status')
    if status['connected'] or status.get('error'): break
    time.sleep(.25)
assert status['connected'], status.get('error')
assert status['runtime']['executionSandbox'] == 'macOSAppSandbox'
assert status['runtime']['codexHome'] == str(pathlib.Path.home() / '.codex')
print('PASS sandboxed Host connected to installed Codex with original Codex Home', flush=True)
account = rpc('account/read', {'refreshToken': False})
assert account.get('account') is not None or account.get('requiresOpenaiAuth') is False
print('PASS original Codex authentication configuration available (identity and credentials omitted)', flush=True)
projects = rpc('project/list', {'limit': 100})
threads = rpc('thread/list', {'limit': 5, 'useStateDbOnly': True})
print('PASS real project/thread list:', len(projects['data']), len(threads['data']), flush=True)
if threads['data']:
    rpc('thread/turns/list', {'threadId': threads['data'][0]['id'], 'limit': 1, 'itemsView': 'full'})
    print('PASS read-only existing history', flush=True)

with tempfile.TemporaryDirectory(prefix='sandbox-execution-', dir=ROOT / '.build') as temporary:
    fixture = pathlib.Path(temporary)
    if args.inference:
        created = rpc('thread/start', {'cwd': temporary, 'ephemeral': True, 'approvalPolicy': 'never'})
        assert created['sandbox'] == {'type': 'externalSandbox', 'networkAccess': 'enabled'}
        thread_id = created['thread']['id']
        status = request('/v1/status')
        stream = WebSocket(port, token, certificate, f"?after={status['lastEventSequence']}&epoch={status['eventEpoch']}")
        stream.socket.settimeout(90)
        commands = []
        try:
            params = {'threadId': thread_id, 'input': [{'type': 'text', 'text':
                'This is an authorized disposable integration test. Use the local command execution tool to run exactly '
                "`printf ANYWHERE_SANDBOX_EXEC_OK > execution-proof.txt; cat execution-proof.txt` in the current working directory. "
                'Do not delegate or use network tools. Do not modify or inspect any other files. '
                'Do not request elevated permissions. If execution fails, report the exact error and stop. '
                'After successful execution reply ANYWHERE_SANDBOX_EXEC_OK.'}]}
            turn = rpc('turn/start', params)
            for _ in range(1000):
                payload = stream.receive()['payload']
                params = payload.get('params', {})
                if params.get('threadId') != thread_id:
                    continue
                if payload['method'] == 'item/completed' and params['item']['type'] == 'commandExecution':
                    commands.append(params['item'])
                    print('Command exit:', params['item'].get('exitCode'), 'output:', params['item'].get('aggregatedOutput', '')[:1800], flush=True)
                if payload['method'] == 'turn/completed' and params['turn']['id'] == turn['turn']['id']:
                    assert params['turn']['status'] == 'completed', params['turn'].get('error')
                    break
            else:
                raise RuntimeError('Inference did not complete')
            assert commands and any(x.get('exitCode') == 0 for x in commands), 'No successful command execution'
            assert (fixture / 'execution-proof.txt').read_text() == 'ANYWHERE_SANDBOX_EXEC_OK'
            print('PASS real model invoked command tool, wrote file, read it, streamed and completed', flush=True)
        finally:
            stream.close()

    def free_port():
        with socket.socket() as s:
            s.bind(('127.0.0.1', 0))
            return s.getsockname()[1]
    control, public = free_port(), free_port()
    with tarfile.open(ROOT / '.build/frp_0.71.1_darwin_arm64.tar.gz') as archive:
        server_path = fixture / 'frps'
        server_path.write_bytes(archive.extractfile('frp_0.71.1_darwin_arm64/frps').read())
        server_path.chmod(0o755)
    config = fixture / 'frps.toml'
    config.write_text(f'bindAddr = "127.0.0.1"\nproxyBindAddr = "127.0.0.1"\nbindPort = {control}\nauth.method = "token"\nauth.token = "sandbox-fixture-token"\ntransport.tls.force = true\nallowPorts = [{{ single = {public} }}]\n')
    with (fixture / 'frps.log').open('w') as log:
        server = subprocess.Popen([str(server_path), '-c', str(config)], stdout=log, stderr=log)
        try:
            for _ in range(50):
                try:
                    with socket.create_connection(('127.0.0.1', control), timeout=.2): break
                except OSError: time.sleep(.1)
            request('/v1/tunnel', {'server': '127.0.0.1', 'serverPort': control, 'remotePort': public, 'token': 'sandbox-fixture-token', 'trustedCaFile': None})
            for _ in range(70):
                try:
                    assert request('/v1/status', listen=public)['connected']
                    break
                except OSError: time.sleep(.1)
            else: raise RuntimeError('Sandboxed frpc forwarding failed')
            print('PASS embedded sandboxed frpc -> real frps -> pinned HTTPS Host', flush=True)
        finally:
            request('/v1/tunnel/stop', {})
            server.terminate()
            server.wait(timeout=8)
