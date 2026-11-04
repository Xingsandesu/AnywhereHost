#!/usr/bin/env python3
"""真实 Codex 只读检查；不创建或修改用户会话。"""
import pathlib,subprocess,tempfile,time
from cargo_artifacts import host_binary
from host_client import HostClient
with tempfile.TemporaryDirectory(prefix='anywhere-readonly-') as tmp:
    state=pathlib.Path(tmp)
    with (state/'log').open('w') as log:
        process=subprocess.Popen([host_binary(),'--state-dir',tmp],stdout=log,stderr=log)
        try:
            client=HostClient(state)
            for _ in range(120):
                try:
                    status=client.request('host.status')
                    if status['connected']:break
                    if status.get('error'):raise RuntimeError(status['error'])
                except OSError:pass
                time.sleep(.25)
            else:raise RuntimeError('Codex timed out')
            for method in ['project/list','thread/list','model/list']:
                value=client.request('codex.call',{'method':method,'params':{'limit':5}})
                assert 'result' in value, value
                print('PASS',method)
        finally:process.terminate();process.wait(timeout=10)
