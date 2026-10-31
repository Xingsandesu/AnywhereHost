#!/usr/bin/env python3
"""Real HTTPS/WSS integration tests against a disposable backend + fake Codex peer."""
import argparse, base64, gzip, hashlib, http.client, json, os, pathlib, socket, ssl, struct, subprocess, tempfile, time
ROOT = pathlib.Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', type=pathlib.Path, default=ROOT / 'target/debug/anywhere-host')
args = parser.parse_args()
binary = str(args.binary.resolve())
passed = 0

def check(condition, name):
    global passed
    assert condition, name
    passed += 1
    print('PASS', name)

from host_wire import WebSocket

with tempfile.TemporaryDirectory(prefix='anywhere-test-') as temporary:
    state = pathlib.Path(temporary); home = state / 'codex'; home.mkdir()
    (home / 'readme.txt').write_text('fixture contents')
    compression_text = 'read-only compression fixture\n' * 1024
    (home / 'compression.txt').write_text(compression_text)
    (state / 'outside.txt').write_text('must not read')
    (home / 'escape').symlink_to(state / 'outside.txt')
    with socket.socket() as listener: listener.bind(('127.0.0.1', 0)); port = listener.getsockname()[1]
    (state / 'config.json').write_text(json.dumps({'listen': f'127.0.0.1:{port}', 'codexHome': str(home), 'codexBinary': str(ROOT / 'Scripts/fake-codex.py')}))
    log = (state / 'host.log').open('w')
    process = subprocess.Popen([binary, '--state-dir', temporary], stdout=log, stderr=log)
    def call(path, body=None, token=None, origin=None):
        connection = http.client.HTTPSConnection('127.0.0.1', port, context=ssl._create_unverified_context(), timeout=10)
        connection.connect()
        cert = ssl.PEM_cert_to_DER_cert((state / 'host-cert.pem').read_text())
        assert connection.sock.getpeercert(binary_form=True) == cert
        headers = {'Content-Type': 'application/json'}
        if token is None: token = (state / 'access-token').read_text()
        if token: headers['Authorization'] = 'Bearer ' + token
        if origin: headers['Origin'] = origin
        connection.request('POST' if body is not None else 'GET', path, json.dumps(body) if body is not None else None, headers)
        response = connection.getresponse(); data = json.loads(response.read()); status = response.status; connection.close()
        return status, data
    def rpc(method, params=None): return call('/v1/rpc', {'method': method, 'params': params or {}})[1]
    try:
        for _ in range(120):
            if process.poll() is not None: raise RuntimeError((state / 'host.log').read_text())
            try:
                _, status = call('/v1/status')
                if status['connected']: break
            except (OSError, FileNotFoundError): pass
            time.sleep(0.1)
        else: raise RuntimeError('Host did not become ready')
        token = (state / 'access-token').read_text()
        certificate = ssl.PEM_cert_to_DER_cert((state / 'host-cert.pem').read_text())
        check(len(token) == 64 and (state / 'access-token').stat().st_mode & 0o777 == 0o600, 'private random credential')
        check(call('/v1/status', token='')[0] == 401, 'unauthenticated HTTP rejected')
        check(call('/v1/status', token='wrong')[0] == 401, 'incorrect bearer rejected')
        check(call('/v1/status', origin='https://hostile.invalid')[0] == 403, 'browser origins rejected')
        check(call('/v1/events', token='')[0] == 401, 'unauthenticated WebSocket rejected before upgrade')
        check(status['certificateSHA256'] == hashlib.sha256(certificate).hexdigest(), 'certificate fingerprint matches TLS peer')
        try:
            connection = http.client.HTTPSConnection('127.0.0.1', port, context=ssl.create_default_context(), timeout=5); connection.connect()
        except ssl.SSLCertVerificationError: verified = True
        else: verified = False; connection.close()
        check(verified, 'unpinned self-signed certificate is rejected')
        check('result' in rpc('project/list'), 'project discovery through actual process protocol')
        check('result' in rpc('thread/list'), 'desktop source filter included')
        check(call('/v1/capabilities')[1]['threadAccess'], 'native client thread access capability advertised')
        check(call('/v1/capabilities')[1]['events']['textDeltaOffsets'] == 'utf8', 'UTF-8 delta positions advertised')
        check(not call('/v1/capabilities')[1]['liveActivityPush']['enabled'], 'APNs is explicitly unavailable without provider credentials')
        check(call('/v1/live-activities/status', token='')[0] == 401, 'APNs status requires paired Host authentication')
        check(call('/v1/live-activities', {'activityId': 'fixture', 'pushToken': 'a' * 64, 'environment': 'sandbox',
              'threadId': 'desktop-1', 'turnId': 'r', 'startedAt': time.time(), 'tokenVersion': 1})[0] == 503,
              'APNs registration never pretends success without signing credentials')
        # Compression negotiates through ordinary HTTP without changing JSON,
        # authentication or the WebSocket upgrade path.
        for encoding in ('gzip', 'identity'):
            compressed = http.client.HTTPSConnection('127.0.0.1', port, context=ssl._create_unverified_context(), timeout=10)
            compressed.connect()
            check(compressed.sock.getpeercert(binary_form=True) == certificate, 'compression probe uses paired certificate')
            compressed.request('GET', '/v1/projects/project-1/file?path=compression.txt', headers={'Authorization': 'Bearer ' + token, 'Accept-Encoding': encoding})
            response = compressed.getresponse(); body = response.read()
            if encoding == 'gzip':
                check(response.getheader('Content-Encoding') == 'gzip', 'Host negotiates gzip')
                plain = gzip.decompress(body)
                check(len(body) < len(plain), 'compressed file response is smaller')
            else:
                check(response.getheader('Content-Encoding') is None, 'identity clients remain compatible')
                plain = body
            check(json.loads(plain)['text'] == compression_text, 'compressed JSON retains protocol semantics')
            compressed.close()
        check(not call('/v1/threads/desktop-1/access')[1]['canMutate'], 'native client receives read-only foreign thread state')
        check(rpc('thread/resume', {'threadId': 'desktop-1'})['error']['code'] == -32009, 'foreign thread cannot be resumed in independent process')
        check(rpc('command/exec', {'command': 'anything'})['error']['code'] == -32601, 'arbitrary command RPC blocked')
        check(rpc('thread/start', {'cwd': 'relative'})['error']['code'] == -32602, 'new thread requires real absolute cwd')
        item_page = rpc('thread/items/list', {'threadId':'desktop-1', 'limit':9999, 'sortDirection':'desc'})['result']
        check(len(item_page['data']) == 100 and item_page['nextCursor'] == '100', 'long single turn is bounded to 100 items')
        next_page = rpc('thread/items/list', {'threadId':'desktop-1', 'cursor':item_page['nextCursor'], 'limit':60, 'sortDirection':'desc'})['result']
        check(len(next_page['data']) == 60 and next_page['data'][0]['item']['id'] == 'item-139', 'item cursor preserves order within the same turn')
        check('result' in rpc('thread/start', {'cwd': str(home)}), 'create native Codex thread')
        # Observe the peer's actual input: no Host policy injection or hidden
        # thread/settings/update is allowed, including after start/fork/resume.
        def forwarded(method, params):
            before = len((home / 'requests.jsonl').read_text().splitlines())
            result = rpc(method, params)
            check('result' in result, f'{method} accepts native permission settings')
            requests = [json.loads(line) for line in (home / 'requests.jsonl').read_text().splitlines()[before:]]
            check(requests == [{'method': method, 'params': params}], f'{method} forwards settings without rewriting or follow-up overrides')
            return result['result']
        forwarded('thread/start', {'cwd': str(home)})
        for mode in ('read-only', 'workspace-write', 'danger-full-access'):
            forwarded('thread/start', {'cwd': str(home), 'sandbox': mode, 'approvalPolicy': 'on-request'})
        forwarded('thread/fork', {'threadId': 'desktop-1', 'sandbox': 'workspace-write', 'approvalPolicy': 'never'})
        forwarded('thread/resume', {'threadId': 'owned-1', 'permissions': 'fixture-profile', 'config': {'sandbox_mode': 'read-only'}})
        settings = {'threadId': 'owned-1', 'sandboxPolicy': {'type': 'dangerFullAccess'}, 'approvalPolicy': 'never'}
        check(forwarded('thread/settings/update', settings) == settings, 'native settings response is preserved')
        empty = rpc('thread/turns/list', {'threadId': 'owned-1'})['result']
        check(empty['data'] == [] and empty['nextCursor'] is None and empty['historyState'] == 'unmaterialized', 'new thread history is an empty first page')
        check('error' in rpc('thread/turns/list', {'threadId': 'owned-1', 'cursor': 'older'}), 'empty-history compatibility does not swallow cursor errors')
        check(rpc('thread/turns/list', {'threadId': 'missing'})['error']['message'] == 'thread not found', 'missing thread still reports original error')
        check('result' in rpc('thread/resume', {'threadId': 'owned-1'}), 'owned thread keeps original identity')
        check(call('/v1/threads/owned-1/access')[1]['canMutate'], 'native client receives writable owned thread state')
        check(rpc('thread/resume', {'threadId': 'owned-1', 'path': '/tmp/substitute'})['error']['code'] == -32602, 'resume path substitution rejected')
        websocket = WebSocket(port, token, certificate)
        first = websocket.until('host/runtime/connected'); cursor = first['sequence']; epoch = first['epoch']
        forwarded('turn/start', {'threadId': 'owned-1', 'input': [{'type': 'text', 'text': 'fixture'}], 'sandboxPolicy': {'type': 'readOnly'}, 'approvalPolicy': 'on-request'})
        approval = websocket.until('host/request/pending'); request_id = approval['payload']['params']['requestId']
        check(len(call('/v1/requests')[1]) == 1, 'pending approvals survive phone reconnect')
        check('error' in call('/v1/requests/' + request_id + '/respond', {'result': {'decision': 'yes'}})[1], 'malformed approval does not consume request')
        check(call('/v1/requests/' + request_id + '/respond', {'result': {'decision': 'accept'}})[1]['result']['accepted'], 'approval returned to correct upstream request')
        delta = websocket.until('item/agentMessage/delta')
        check(delta['payload']['params']['hostTextOffset'] == 0, 'streamed delta has exact text position')
        complete = websocket.until('turn/completed')
        check(complete['payload']['params']['turn']['status'] == 'completed', 'authoritative completion event delivered')
        history = rpc('thread/turns/list', {'threadId': 'owned-1'})['result']
        check(history['data'][0]['id'] == 'turn-1' and 'historyState' not in history, 'first turn materializes without an empty snapshot cache')
        check(call('/v1/requests/' + request_id + '/respond', {'result': {'decision': 'accept'}})[1]['error']['code'] == -32004, 'approval is single use')
        websocket.close()
        resumed = WebSocket(port, token, certificate, f'?after={cursor}&epoch={epoch}')
        replay_delta = resumed.until('item/agentMessage/delta')
        check(replay_delta == delta, 'delta positions and sequence survive journal replay')
        replay = resumed.until('turn/completed'); resumed.close()
        check(replay['sequence'] == complete['sequence'], 'WSS cursor replay preserves event sequence')
        # Cursor validation is checked by a full Upgrade request.
        try: WebSocket(port, token, certificate, '?after=1&epoch=wrong')
        except AssertionError as error: invalid = '409' in str(error)
        else: invalid = False
        check(invalid, 'invalid epoch requires resync')
        check(call('/v1/projects/project-1/file?path=readme.txt')[1]['text'] == 'fixture contents', 'project text read')
        check(call('/v1/projects/project-1/file?path=../outside.txt')[0] == 400, 'parent traversal rejected')
        check(call('/v1/projects/project-1/file?path=escape')[0] == 400, 'symlink escape rejected')
        duplicate = subprocess.run([binary, '--state-dir', temporary], capture_output=True, timeout=5)
        check(duplicate.returncode != 0 and b'already running' in duplicate.stderr, 'duplicate host cannot acquire state lock')
    finally:
        process.terminate()
        try: process.wait(timeout=8)
        except subprocess.TimeoutExpired: process.kill(); process.wait()
        log.close()
print(f'{passed} integration checks passed')
