#!/usr/bin/env python3
"""Read-only smoke against the user's installed Codex. No model inference or existing thread mutation."""
import hashlib, http.client, json, pathlib, socket, ssl, subprocess, tempfile, time, sys
ROOT = pathlib.Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix='anywhere-real-') as temp:
    directory = pathlib.Path(temp)
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0)); port = s.getsockname()[1]
    (directory / 'config.json').write_text(json.dumps({'listen': f'127.0.0.1:{port}'}))
    log = (directory / 'host.log').open('w')
    process = subprocess.Popen([str(ROOT / 'target/debug/anywhere-host'), '--state-dir', temp], stdout=log, stderr=log)
    def request(path, body=None):
        conn = http.client.HTTPSConnection('127.0.0.1', port, context=ssl._create_unverified_context(), timeout=70)
        conn.connect()
        expected = ssl.PEM_cert_to_DER_cert((directory / 'host-cert.pem').read_text())
        assert hashlib.sha256(conn.sock.getpeercert(binary_form=True)).digest() == hashlib.sha256(expected).digest()
        headers = {'Authorization': 'Bearer ' + (directory / 'access-token').read_text(), 'Content-Type': 'application/json'}
        conn.request('POST' if body is not None else 'GET', path, json.dumps(body) if body is not None else None, headers)
        response = conn.getresponse(); data = json.loads(response.read()); conn.close()
        assert response.status == 200, data
        return data
    try:
        for _ in range(80):
            if process.poll() is not None: raise RuntimeError((directory / 'host.log').read_text())
            try:
                status = request('/v1/status')
                if status['connected']: break
                if status.get('error'): raise RuntimeError(status['error'])
            except (OSError, FileNotFoundError): pass
            time.sleep(0.25)
        else: raise RuntimeError('Codex handshake timed out')
        print('PASS actual Codex initialize:', status['runtime']['mode'])
        def rpc(method, params):
            value = request('/v1/rpc', {'method': method, 'params': params})
            assert 'error' not in value, value
            return value['result']
        projects = rpc('project/list', {'limit': 100})
        print('PASS project/list:', len(projects['data']), 'projects')
        threads = rpc('thread/list', {'limit': 5, 'sortKey': 'updated_at', 'useStateDbOnly': True})
        print('PASS thread/list:', len(threads['data']), 'threads')
        if threads['data']:
            thread = threads['data'][0]
            history = rpc('thread/turns/list', {'threadId': thread['id'], 'limit': 1, 'itemsView': 'full'})
            print('PASS thread/turns/list:', len(history['data']), 'turns')
            deny = request('/v1/rpc', {'method': 'thread/resume', 'params': {'threadId': thread['id'], 'excludeTurns': True}})
            if not status['runtime']['sharesLiveProcess']:
                assert deny['error']['code'] == -32009
                print('PASS existing desktop thread mutation is blocked')
        models = rpc('model/list', {'limit': 100})
        print('PASS model/list:', len(models['data']), 'models')
        if projects['data']:
            project = projects['data'][0]
            files = request('/v1/projects/' + project['id'] + '/files')
            print('PASS rooted project files:', len(files['entries']), 'entries')
        print('PASS read-only real Codex smoke')
        if '--inference' in sys.argv or '--first-message' in sys.argv:
            # Ephemeral thread avoids adding fixture conversations to the user's saved history.
            # --first-message explicitly exercises persistent history in a NEW
            # disposable thread, then archives only that test-owned thread.
            persistent = '--first-message' in sys.argv
            created = rpc('thread/start', {'cwd': temp, 'ephemeral': not persistent, 'approvalPolicy': 'never', 'sandbox': 'read-only'})
            thread_id = created['thread']['id']
            from host_wire import WebSocket
            status = request('/v1/status')
            stream = WebSocket(port, (directory / 'access-token').read_text(), ssl.PEM_cert_to_DER_cert((directory / 'host-cert.pem').read_text()), f"?after={status['lastEventSequence']}&epoch={status['eventEpoch']}")
            stream.socket.settimeout(60)
            try:
                if persistent:
                    assert request('/v1/threads/' + thread_id + '/access')['canMutate']
                    assert rpc('thread/read', {'threadId': thread_id})['thread']['id'] == thread_id
                    before = rpc('thread/turns/list', {'threadId': thread_id, 'limit': 20, 'itemsView': 'full', 'sortDirection': 'desc'})
                    assert before['data'] == [], before
                    print('PASS real empty thread metadata/access/history before first message; no unnecessary resume')
                turn = rpc('turn/start', {'threadId': thread_id, 'input': [{'type': 'text', 'text': 'Reply exactly ANYWHERE_HOST_OK. Do not call any tools or modify any files.'}]})
                turn_id = turn['turn']['id']; texts = []
                for _ in range(500):
                    event = stream.receive()['payload']; params = event.get('params', {})
                    if params.get('threadId') != thread_id: continue
                    if event['method'] == 'item/agentMessage/delta': texts.append(params['delta'])
                    if event['method'] == 'turn/completed' and params['turn']['id'] == turn_id:
                        assert params['turn']['status'] == 'completed', params['turn'].get('error', params['turn']['status'])
                        assert 'ANYWHERE_HOST_OK' in ''.join(texts), 'Missing expected model reply'
                        print('PASS real Codex turn streamed and completed with expected reply')
                        if persistent:
                            after = rpc('thread/turns/list', {'threadId': thread_id, 'limit': 20, 'itemsView': 'full'})
                            assert any(t['id'] == turn_id for t in after['data']), 'First turn missing from materialized history'
                            print('PASS real first turn is available through paginated history')
                        break
                else: raise RuntimeError('Real inference timed out')
            finally:
                stream.close()
                if persistent:
                    rpc('thread/archive', {'threadId': thread_id})
                    print('PASS archived only the newly created regression thread')
    finally:
        process.terminate()
        try: process.wait(timeout=8)
        except subprocess.TimeoutExpired: process.kill(); process.wait()
        log.close()
