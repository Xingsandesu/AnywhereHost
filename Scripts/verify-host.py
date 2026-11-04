#!/usr/bin/env python3
"""使用隔离 Codex 替身验证本机管理和业务策略。"""
import argparse,json,pathlib,subprocess,sys,tempfile,time,uuid
from cargo_artifacts import host_binary
ROOT=pathlib.Path(__file__).resolve().parents[1]
from host_client import HostClient
parser=argparse.ArgumentParser();parser.add_argument('--binary',default=None);args=parser.parse_args()
args.binary=args.binary or host_binary()
expected_version, expected_build = subprocess.check_output([sys.executable, str(ROOT/"Scripts/version.py")], text=True).strip().split()
with tempfile.TemporaryDirectory(prefix='anywhere-') as tmp:
    state=pathlib.Path(tmp);home=state/'codex';home.mkdir()
    (state/'config.json').write_text(json.dumps({'codexHome':str(home),'codexBinary':str(ROOT/'Scripts/fake-codex.py')}))
    with (state/'log').open('w') as log:
        process=subprocess.Popen([args.binary,'--state-dir',tmp],stdout=log,stderr=log)
        try:
            client=HostClient(state)
            for _ in range(100):
                try:
                    status=client.request('host.status')
                    if status['connected']:break
                except OSError:pass
                if process.poll() is not None:raise RuntimeError((state/'log').read_text())
                time.sleep(.1)
            else:raise RuntimeError('Codex fixture timed out')
            assert status['version']==expected_version and status['build']==int(expected_build.strip('()')) and status['protocolVersion']==3
            assert (state/'identity.key').stat().st_mode & 0o777 == 0o600
            assert (state/'control.sock').stat().st_mode & 0o777 == 0o600
            def rpc(method,params):return client.request('codex.call',{'method':method,'params':params})['result']
            assert rpc('project/list',{})['data'][0]['id']=='project-1'
            assert not client.request('thread.access',{'threadId':'desktop-1'})['canMutate']
            assert rpc('thread/fork',{'threadId':'desktop-1'})['thread']['id']=='owned-1'
            assert client.request('thread.access',{'threadId':'owned-1'})['canMutate']
            assert 'entries' in client.request('resource.files',{'projectId':'project-1'})
            try:rpc('command/exec',{'command':'whoami'})
            except RuntimeError:pass
            else:raise AssertionError('Unlisted method accepted')
            try: client.request('pairing.create')
            except RuntimeError: pass
            else: raise AssertionError('Unattested Host exposed pairing credentials')
            print('PASS isolated runtime, identity, resources, ownership and method policy')
        finally:
            process.terminate();process.wait(timeout=10)
