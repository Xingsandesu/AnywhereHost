#!/usr/bin/env python3
from security_wire import admin_headers
"""Verify shared-socket attachment without touching the user's Codex Home or running inference."""
import http.client,json,os,pathlib,socket,ssl,subprocess,tempfile,time
ROOT=pathlib.Path(__file__).resolve().parents[1]
BINARY=pathlib.Path('/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex')
if not BINARY.exists(): raise SystemExit('Set BINARY to an installed current Codex runtime')
with tempfile.TemporaryDirectory(prefix='ah-shared-',dir='/tmp') as temporary:
    d=pathlib.Path(temporary); home=d/'codex';home.mkdir();state=d/'host';state.mkdir();sock=d/'rpc.sock'
    env={**os.environ,'CODEX_HOME':str(home)}
    with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
    (state/'config.json').write_text(json.dumps({'listen':f'127.0.0.1:{port}','codexHome':str(home),'codexBinary':str(BINARY),'controlSocket':str(sock)}))
    log=(d/'log').open('w');daemon=subprocess.Popen([str(BINARY),'app-server','--listen',f'unix://{sock}'],env=env,stdin=subprocess.DEVNULL,stdout=log,stderr=log);host=None
    try:
        for _ in range(50):
            if sock.exists():break
            time.sleep(.1)
        assert sock.exists()
        host=subprocess.Popen([str(ROOT/'target/debug/anywhere-host'),'--state-dir',str(state)],stdout=log,stderr=log)
        def call(path,body=None):
            c=http.client.HTTPSConnection('127.0.0.1',port,context=ssl._create_unverified_context(),timeout=5);c.connect()
            assert c.sock.getpeercert(binary_form=True)==ssl.PEM_cert_to_DER_cert((state/'host-cert.pem').read_text())
            c.request('GET' if body is None else 'POST',path,None if body is None else json.dumps(body),{**admin_headers(port,(state/'admin-key-v2').read_text(),ssl.PEM_cert_to_DER_cert((state/'host-cert.pem').read_text()),'GET' if body is None else 'POST',path,None if body is None else json.dumps(body)),'Content-Type':'application/json'})
            r=c.getresponse();value=json.loads(r.read());c.close();assert r.status==200,value;return value
        for _ in range(100):
            try:
                status=call('/v1/status')
                if status['connected']:break
                if status['error']:raise RuntimeError(status['error'])
            except OSError:pass
            time.sleep(.1)
        else:raise RuntimeError('shared handshake timed out')
        assert status['runtime']['sharesLiveProcess'] and status['runtime']['mode']=='sharedCodexProcess'
        assert 'result' in call('/v1/rpc',{'method':'project/list','params':{}})
        print('PASS existing real Codex Unix socket attaches through native WebSocket')
        host.terminate();host.wait(timeout=8)
        assert daemon.poll() is None
        print('PASS Host shutdown leaves external Codex daemon running')
    finally:
        for proc in [host,daemon]:
            if proc and proc.poll() is None:
                proc.terminate()
                try:proc.wait(timeout=8)
                except subprocess.TimeoutExpired:proc.kill();proc.wait()
        log.close()
