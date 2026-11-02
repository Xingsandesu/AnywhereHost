#!/usr/bin/env python3
from security_wire import admin_headers
"""Real frpc/frps regression checks. All listeners and connections use loopback."""
import http.client
import json
import pathlib
import platform
import socket
import ssl
import subprocess
import tarfile
import tempfile
import time
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[1]
ARCH = {'arm64': 'arm64', 'aarch64': 'arm64', 'x86_64': 'amd64'}[platform.machine()]
SYSTEM = platform.system().lower()
TOKEN = 'local-fixture-token'


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


with tempfile.TemporaryDirectory(prefix='anywhere-frp-') as temporary:
    directory = pathlib.Path(temporary)
    ports = set()
    while len(ports) < 6:
        ports.add(free_port())
    host_port, control_port, public_port, occupied_port, second_host_port, second_public_port = ports
    (directory / 'config.json').write_text(json.dumps({
        'listen': f'127.0.0.1:{host_port}', 'codexHome': temporary,
        'codexBinary': str(ROOT / 'Scripts/fake-codex.py'),
        'frpcBinary': str(ROOT / 'Resources/frpc'),
    }))
    with tarfile.open(ROOT / f'.build/frp_0.71.1_{SYSTEM}_{ARCH}.tar.gz') as archive:
        (directory / 'frps').write_bytes(archive.extractfile(f'frp_0.71.1_{SYSTEM}_{ARCH}/frps').read())
        (directory / 'frps').chmod(0o755)
    logs = [(directory / 'host.log').open('w'), (directory / 'server.log').open('w')]
    host = subprocess.Popen([str(ROOT / 'target/debug/anywhere-host'), '--state-dir', temporary], stdout=logs[0], stderr=logs[0])
    server = None
    second_host = None

    def call(listen, path, body=None, expected=200, state_directory=directory):
        connection = http.client.HTTPSConnection('127.0.0.1', listen, context=ssl._create_unverified_context(), timeout=3)
        connection.connect()
        certificate = ssl.PEM_cert_to_DER_cert((state_directory / 'host-cert.pem').read_text())
        assert connection.sock.getpeercert(binary_form=True) == certificate
        connection.request('GET' if body is None else 'POST', path, None if body is None else json.dumps(body), {
            **admin_headers(listen,(state_directory/'admin-key-v2').read_text(),certificate,'GET' if body is None else 'POST',path,None if body is None else json.dumps(body)), 'Content-Type': 'application/json',
        })
        response = connection.getresponse()
        value = json.loads(response.read())
        connection.close()
        assert response.status in (expected if isinstance(expected, tuple) else (expected,)), (response.status, value)
        return value

    def config(**overrides):
        return dict({'server': '127.0.0.1', 'serverPort': control_port, 'remotePort': public_port,
                     'token': TOKEN, 'trustedCaFile': None}, **overrides)

    def wait_for(predicate, message):
        for _ in range(70):
            try:
                if predicate():
                    return
            except (OSError, http.client.HTTPException):
                pass
            time.sleep(.1)
        raise AssertionError(message)

    def start_server(custom_certificate):
        text = f'''bindAddr = "127.0.0.1"
proxyBindAddr = "127.0.0.1"
bindPort = {control_port}
auth.method = "token"
auth.token = "{TOKEN}"
transport.tls.force = true
allowPorts = [{{ single = {public_port} }}, {{ single = {occupied_port} }}, {{ single = {second_public_port} }}]
'''
        if custom_certificate:
            text += f'transport.tls.certFile = {json.dumps(str(directory / "host-cert.pem"))}\n'
            text += f'transport.tls.keyFile = {json.dumps(str(directory / "host-key.pem"))}\n'
        (directory / 'frps.toml').write_text(text)
        process = subprocess.Popen([str(directory / 'frps'), '-c', str(directory / 'frps.toml')], stdout=logs[1], stderr=logs[1])
        def listening():
            assert process.poll() is None, (directory / 'server.log').read_text()
            with socket.create_connection(('127.0.0.1', control_port), timeout=.2):
                return True
        try:
            wait_for(listening, 'frps did not start listening')
        except Exception:
            process.terminate()
            process.wait(timeout=8)
            raise
        return process

    def wait_public():
        wait_for(lambda: call(public_port, '/v1/status')['connected'], 'forwarded Host unavailable')

    def stop_tunnel():
        assert call(host_port, '/v1/tunnel/stop', {})['state'] == 'stopped'
        time.sleep(.15)

    try:
        wait_for(lambda: call(host_port, '/v1/status')['connected'], 'Host failed to start')
        server = start_server(False)
        call(host_port, '/v1/tunnel', config())
        wait_public()
        state = call(host_port, '/v1/tunnel')
        assert state['state'] == 'running' and state['config']['hasToken']
        assert 'login to server success' in state['logTail'] and 'start proxy success' in state['logTail']
        assert TOKEN not in json.dumps(state) and 'token' not in state['config']
        assert (directory / 'frpc.toml').stat().st_mode & 0o777 == 0o600
        assert (directory / 'frpc.log').stat().st_mode & 0o777 == 0o600
        print('PASS default frp TLS without CA: real forwarding, private config, redacted diagnostics')

        # Two real Host identities must coexist on the same frps, with separate ports.
        first_name = tomllib.loads((directory / 'frpc.toml').read_text())['proxies'][0]['name']
        second_directory = directory / 'second-host'
        second_directory.mkdir()
        (second_directory / 'config.json').write_text(json.dumps({
            'listen': f'127.0.0.1:{second_host_port}', 'codexHome': str(second_directory),
            'codexBinary': str(ROOT / 'Scripts/fake-codex.py'), 'frpcBinary': str(ROOT / 'Resources/frpc'),
        }))
        logs.append((second_directory / 'host.log').open('w'))
        second_host = subprocess.Popen([str(ROOT / 'target/debug/anywhere-host'), '--state-dir', str(second_directory)],
                                       stdout=logs[-1], stderr=logs[-1])
        wait_for(lambda: call(second_host_port, '/v1/status', state_directory=second_directory)['connected'], 'second Host failed to start')
        call(second_host_port, '/v1/tunnel', config(remotePort=second_public_port), state_directory=second_directory)
        wait_for(lambda: call(second_public_port, '/v1/status', state_directory=second_directory)['connected'], 'second Host proxy unavailable')
        wait_public()
        second_name = tomllib.loads((second_directory / 'frpc.toml').read_text())['proxies'][0]['name']
        assert first_name != second_name and all(name.startswith('anywhere-host-') for name in (first_name, second_name))
        call(second_host_port, '/v1/tunnel/stop', {}, state_directory=second_directory)
        second_host.terminate()
        second_host.wait(timeout=8)
        print('PASS two Host identities forward concurrently through the same frps without proxy-name collisions')
        stop_tunnel()

        # Restart Host: settings survive and a blank token can reuse the same frps credential.
        # An old fixed-name config is upgraded without changing connection settings.
        saved = directory / 'frpc.toml'
        saved.write_text(saved.read_text().replace(first_name, 'anywhere-host'))
        host.terminate()
        host.wait(timeout=8)
        host = subprocess.Popen([str(ROOT / 'target/debug/anywhere-host'), '--state-dir', temporary], stdout=logs[0], stderr=logs[0])
        wait_for(lambda: call(host_port, '/v1/status')['connected'], 'Host restart failed')
        assert call(host_port, '/v1/tunnel')['config']['serverPort'] == control_port
        call(host_port, '/v1/tunnel', config(token=''))
        wait_public()
        assert tomllib.loads(saved.read_text())['proxies'][0]['name'] == first_name
        stop_tunnel()
        assert '重新输入 token' in call(host_port, '/v1/tunnel', config(token='', serverPort=free_port()), 400)['error']
        assert '必须不同' in call(host_port, '/v1/tunnel', config(remotePort=control_port), 400)['error']
        print('PASS legacy proxy-name migration, stable identity, configuration recovery and restricted credential reuse')

        with socket.socket() as blocker:
            blocker.bind(('127.0.0.1', occupied_port))
            blocker.listen()
            call(host_port, '/v1/tunnel', config(remotePort=occupied_port))
            wait_for(lambda: 'start error' in call(host_port, '/v1/tunnel')['logTail'], 'missing proxy bind failure')
            stop_tunnel()
        print('PASS public-port conflict appears in tunnel diagnostics')

        # A failed login may exit before or after POST returns, depending on timing.
        call(host_port, '/v1/tunnel', config(token='wrong-fixture-token'), (200, 400))
        wait_for(lambda: call(host_port, '/v1/tunnel')['state'] == 'exited', 'wrong token did not exit')
        state = call(host_port, '/v1/tunnel')
        assert 'token' in state['logTail'].lower() and 'wrong-fixture-token' not in json.dumps(state)
        stop_tunnel()
        print('PASS failed token authentication has visible redacted diagnostics')

        server.terminate()
        server.wait(timeout=8)
        server = start_server(True)
        call(host_port, '/v1/tunnel', config(trustedCaFile=str(directory / 'host-cert.pem')))
        wait_public()
        stop_tunnel()
        print('PASS custom CA verified frps connection and pinned Host HTTPS forwarding')
    finally:
        for process in [host, second_host, server]:
            if process and process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
        for log in logs:
            log.close()
