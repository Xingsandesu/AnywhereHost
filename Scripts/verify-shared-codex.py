#!/usr/bin/env python3
"""Verify shared-socket attachment without touching the user's Codex Home or running inference."""
import argparse,http.client,json,os,pathlib,shutil,socket,ssl,subprocess,tempfile,time
from cargo_artifacts import host_binary
from host_client import HostClient
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--codex-binary',type=pathlib.Path)
args=parser.parse_args()
app=pathlib.Path('/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex')
BINARY=args.codex_binary or (app if app.exists() else pathlib.Path(shutil.which('codex') or '/nonexistent-codex'))
if not BINARY.is_file(): raise SystemExit('Pass --codex-binary with an installed current Codex runtime')
with tempfile.TemporaryDirectory(prefix='ah-shared-',dir='/tmp') as temporary:
    d=pathlib.Path(temporary); home=d/'codex';home.mkdir();state=d/'host';state.mkdir();sock=d/'rpc.sock'
    env={**os.environ,'CODEX_HOME':str(home)}
    with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
    (state/'config.json').write_text(json.dumps({'codexHome':str(home),'codexBinary':str(d/'missing-unrelated-binary'),'controlSocket':str(sock)}))
    log=(d/'log').open('w');daemon=subprocess.Popen([str(BINARY),'app-server','--listen',f'unix://{sock}'],env=env,stdin=subprocess.DEVNULL,stdout=log,stderr=log);host=None
    try:
        for _ in range(50):
            if sock.exists():break
            time.sleep(.1)
        assert sock.exists()
        host=subprocess.Popen([host_binary(),'--state-dir',str(state)],stdout=log,stderr=log)
        client=HostClient(state)
        call=client.request
        for _ in range(100):
            try:
                status=call('host.status')
                if status['connected']:break
                if status['error']:raise RuntimeError(status['error'])
            except OSError:pass
            time.sleep(.1)
        else:raise RuntimeError('shared handshake timed out')
        assert pathlib.Path(status['runtime']['binary']).resolve() == BINARY.resolve()
        assert status['runtime']['sharesLiveProcess'] and status['runtime']['mode']=='sharedCodexProcess'
        assert 'result' in call('codex.call',{'method':'project/list','params':{}})
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
