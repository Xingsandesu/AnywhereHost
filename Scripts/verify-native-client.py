#!/usr/bin/env python3
"""编译实际 Swift 客户端，并通过本机 relay 完成隔离双端检查。"""
import argparse,json,os,pathlib,signal,subprocess,sys,tempfile,time
from cargo_artifacts import build
ROOT=pathlib.Path(__file__).resolve().parents[1]
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--client-root',type=pathlib.Path,default=os.environ.get('ANYWHERE_CLIENT_SOURCE'))
args=parser.parse_args()
if not args.client_root:parser.error('Set --client-root or ANYWHERE_CLIENT_SOURCE')
CLIENT=args.client_root.resolve()
if not (CLIENT/'Scripts/HostFixtures/HostTransportSmoke.swift').is_file():parser.error('Invalid client checkout')
from host_client import HostClient
transport=build('--locked','-p','anywhere-transport','--lib')
library=next(path for path in transport['anywhere_transport']['filenames'] if path.endswith('.a'))
artifacts=build('--locked','-p','anywhere-host','--bins','--example','relay-fixture')
with tempfile.TemporaryDirectory(prefix='anywhere-native-') as tmp:
    state=pathlib.Path(tmp);home=state/'codex';home.mkdir();(home/'fixture.txt').write_text('native resource')
    with (state/'log').open('w') as log:
        relay=subprocess.Popen([artifacts['relay-fixture']['executable']],stdout=subprocess.PIPE,stderr=log,text=True)
        host=None
        try:
            url=relay.stdout.readline().strip()
            if not url.startswith('http://127.0.0.1:'):raise RuntimeError('relay did not start')
            (state/'config.json').write_text(json.dumps({'relayUrls':[url],'codexHome':str(home),'codexBinary':str(ROOT/'Scripts/fake-codex.py')}))
            host=subprocess.Popen([artifacts['anywhere-host']['executable'],'--state-dir',tmp,'--test-relay',url],stdout=log,stderr=log)
            client=HostClient(state)
            for _ in range(100):
                try:
                    status=client.request('host.status')
                    if status['connected']:break
                except OSError:pass
                time.sleep(.1)
            else:raise RuntimeError('host did not start')
            sources=[CLIENT/'AnyWhereCoding'/p for p in ['Models/HostJSON.swift','Models/HostConnection.swift','Services/KeychainStore.swift','Services/Host/HostIdentity.swift','Services/Host/HostTransport.swift']]
            sources.append(ROOT/'transport/apple/NetworkAdmission.swift')
            executable=state/'client-smoke'
            subprocess.run(['swiftc','-D','ANYWHERE_NETWORK_TEST','-swift-version','6','-parse-as-library',*map(str,sources),str(CLIENT/'Scripts/HostFixtures/HostTransportSmoke.swift'),library,'-framework','Security','-framework','SystemConfiguration','-o',str(executable)],check=True)
            (state/'pairing.json').write_text(json.dumps(client.request('pairing.create')))
            subprocess.run([str(executable),str(state/'pairing.json')],check=True,timeout=60)
        finally:
            for p in [host,relay]:
                if p and p.poll() is None:
                    p.send_signal(signal.SIGTERM)
                    try:p.wait(timeout=8)
                    except subprocess.TimeoutExpired:p.kill();p.wait()
