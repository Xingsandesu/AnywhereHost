#!/usr/bin/env python3
"""Local-only real frpc/frps round trip. Both listeners bind to 127.0.0.1."""
import hashlib,http.client,json,pathlib,platform,socket,ssl,subprocess,tarfile,tempfile,time
ROOT=pathlib.Path(__file__).resolve().parents[1]
arch={'arm64':'arm64','x86_64':'amd64'}[platform.machine()]

def port():
    with socket.socket() as s: s.bind(('127.0.0.1',0)); return s.getsockname()[1]
with tempfile.TemporaryDirectory(prefix='anywhere-frp-') as temporary:
    d=pathlib.Path(temporary); host_port=port(); control_port=port(); public_port=port()
    (d/'config.json').write_text(json.dumps({'listen':f'127.0.0.1:{host_port}','codexHome':str(d),'codexBinary':str(ROOT/'Scripts/fake-codex.py'),'frpcBinary':str(ROOT/'Resources/frpc')}))
    with tarfile.open(ROOT/f'.build/frp_0.71.1_darwin_{arch}.tar.gz') as archive:
        (d/'frps').write_bytes(archive.extractfile(f'frp_0.71.1_darwin_{arch}/frps').read()); (d/'frps').chmod(0o755)
    logs=[(d/'host.log').open('w'),(d/'server.log').open('w')]
    host=subprocess.Popen([str(ROOT/'target/debug/anywhere-host'),'--state-dir',temporary],stdout=logs[0],stderr=logs[0]);server=None
    def call(listen,path,body=None):
        c=http.client.HTTPSConnection('127.0.0.1',listen,context=ssl._create_unverified_context(),timeout=3);c.connect()
        assert c.sock.getpeercert(binary_form=True)==ssl.PEM_cert_to_DER_cert((d/'host-cert.pem').read_text())
        c.request('GET' if body is None else 'POST',path,None if body is None else json.dumps(body),{'Authorization':'Bearer '+(d/'access-token').read_text(),'Content-Type':'application/json'})
        r=c.getresponse();value=json.loads(r.read());c.close();assert r.status==200,value;return value
    try:
        for _ in range(100):
            try:
                if call(host_port,'/v1/status')['connected']: break
            except OSError:pass
            time.sleep(.1)
        else:raise RuntimeError('host failed')
        (d/'frps.toml').write_text(f'''bindAddr = "127.0.0.1"
proxyBindAddr = "127.0.0.1"
bindPort = {control_port}
auth.method = "token"
auth.token = "local-fixture-token"
transport.tls.force = true
transport.tls.certFile = {json.dumps(str(d/'host-cert.pem'))}
transport.tls.keyFile = {json.dumps(str(d/'host-key.pem'))}
allowPorts = [{{ single = {public_port} }}]
''')
        server=subprocess.Popen([str(d/'frps'),'-c',str(d/'frps.toml')],stdout=logs[1],stderr=logs[1])
        time.sleep(.4)
        call(host_port,'/v1/tunnel',{'server':'localhost','serverPort':control_port,'remotePort':public_port,'token':'local-fixture-token','trustedCaFile':str(d/'host-cert.pem')})
        for _ in range(50):
            try:
                status=call(public_port,'/v1/status');break
            except OSError:time.sleep(.2)
        else:raise RuntimeError('tunnel unreachable: '+(d/'frpc.log').read_text())
        assert status['connected']
        print('PASS pinned HTTPS API through bundled frpc and real local frps')
        assert call(host_port,'/v1/tunnel')['state']=='running'
        assert call(host_port,'/v1/tunnel/stop',{})['state']=='stopped'
        print('PASS structured tunnel start/status/stop')
    finally:
        for proc in [host,server]:
            if proc:
                proc.terminate()
                try:proc.wait(timeout=8)
                except subprocess.TimeoutExpired:proc.kill();proc.wait()
        for log in logs:log.close()
