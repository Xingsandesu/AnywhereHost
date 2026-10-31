#!/usr/bin/env python3
"""Attack-oriented TLS + proof/enrollment regression on an isolated Host. No real Codex."""
import base64, hashlib, http.client, json, pathlib, socket, ssl, subprocess, tempfile, time
from security_wire import admin_headers, challenge, connection, Device
from host_wire import WebSocket
ROOT=pathlib.Path(__file__).resolve().parents[1]
checks=0
def check(value,label):
    global checks
    assert value,label
    checks+=1; print('PASS',label,flush=True)
with tempfile.TemporaryDirectory(prefix='anywhere-security-') as temporary:
    state=pathlib.Path(temporary); home=state/'codex';home.mkdir()
    with socket.socket() as s: s.bind(('127.0.0.1',0));port=s.getsockname()[1]
    (state/'config.json').write_text(json.dumps({'listen':f'127.0.0.1:{port}','codexHome':str(home),'codexBinary':str(ROOT/'Scripts/fake-codex.py')}))
    log=(state/'host.log').open('w')
    def start(): return subprocess.Popen([str(ROOT/'target/debug/anywhere-host'),'--state-dir',temporary],stdout=log,stderr=log)
    process=start()
    def ready():
        for _ in range(120):
            if process.poll() is not None: raise RuntimeError((state/'host.log').read_text())
            try:
                cert=ssl.PEM_cert_to_DER_cert((state/'host-cert.pem').read_text()); challenge(port,cert);return cert
            except (OSError,ValueError): time.sleep(.1)
        raise RuntimeError('startup failed')
    def call(path='/v1/status',method='GET',body=None,headers=None):
        c=connection(port,cert)
        try:
            c.request(method,path,body,headers or {}); r=c.getresponse(); raw=r.read()
            return r.status,json.loads(raw),dict(r.getheaders())
        finally: c.close()
    def admin(path,method='GET',body=None):
        body=json.dumps(body) if body is not None else None
        return call(path,method,body,{**admin_headers(port,key,cert,method,path,body),'Content-Type':'application/json'})
    try:
        cert=ready();key=(state/'admin-key-v2').read_text()
        c=connection(port,cert);check(c.sock.version()=='TLSv1.3','TLS 1.3 negotiated with pinned self-signed certificate');c.close()
        context=ssl._create_unverified_context();context.maximum_version=ssl.TLSVersion.TLSv1_2
        try: context.wrap_socket(socket.create_connection(('127.0.0.1',port)),server_hostname='localhost')
        except ssl.SSLError: rejected=True
        else: rejected=False
        check(rejected,'TLS 1.2 rejected')
        check(call(headers={'Authorization':'Bearer '+key})[0]==401,'legacy bearer rejected, including correct local secret')
        code,_,headers=call();check(code==401 and headers['cache-control']=='no-store','unauthenticated rejection is non-cacheable')
        check(call(headers={'Origin':'null'})[0]==403,'opaque browser origin rejected')
        check(call('/v1/status?token='+key)[0]==401,'query credential cannot authenticate')
        d=Device(state)
        proof=d.headers(port,cert,'GET','/v1/status')
        check(call(headers=proof)[0]==401,'unregistered device rejected')
        check(call('/v1/security/pair','POST',headers=d.headers(port,cert,'POST','/v1/security/pair',ticket='0'*64))[0]==403,'unissued ticket rejected before replay-state allocation')
        ticket=admin('/v1/security/ticket','POST',{})[1]['ticket']
        enroll=d.headers(port,cert,'POST','/v1/security/pair',ticket=ticket)
        check(call('/v1/security/pair','POST',headers=enroll)[1]['deviceID']==d.id,'one scan enrolls independently generated P-256 device key')
        check(call('/v1/security/pair','POST',headers=enroll)[0]==401,'enrollment signature replay rejected')
        check(call('/v1/security/pair','POST',headers=d.headers(port,cert,'POST','/v1/security/pair',ticket=ticket))[0]==200,'same key recovers lost enrollment response')
        otherdir=state/'other';otherdir.mkdir();other=Device(otherdir)
        check(call('/v1/security/pair','POST',headers=other.headers(port,cert,'POST','/v1/security/pair',ticket=ticket))[0]==403,'consumed QR cannot enroll a second key')
        proof=d.headers(port,cert,'GET','/v1/status')
        check(call(headers=proof)[0]==200,'enrolled device can reconnect')
        check(call(headers=proof)[0]==401,'exact request replay rejected')
        duplicate=d.headers(port,cert,'GET','/v1/status')
        c=connection(port,cert);c.putrequest('GET','/v1/status')
        for name,value in duplicate.items(): c.putheader(name,value)
        c.putheader('X-Anywhere-Nonce',duplicate['X-Anywhere-Nonce']);c.endheaders()
        r=c.getresponse();check(r.status==401,'duplicate proof headers rejected');r.read();c.close()
        forged=d.headers(port,cert,'GET','/v1/status');forged['X-Anywhere-Signature']=base64.b64encode(bytes(64)).decode()
        check(call(headers=forged)[0]==401,'forged signature rejected')
        check(call('/v1/capabilities',headers=d.headers(port,cert,'GET','/v1/status'))[0]==401,'signature bound to route')
        check(call('/v1/status?after=2',headers=d.headers(port,cert,'GET','/v1/status?after=1'))[0]==401,'signature bound to query')
        check(call('/v1/status','POST',headers=d.headers(port,cert,'GET','/v1/status'))[0]==401,'signature bound to method')
        check(call('/v1/rpc','POST','{}',d.headers(port,cert,'POST','/v1/rpc','{"method":"x"}'))[0]==401,'body substitution rejected before RPC dispatch')
        check(call(headers=d.headers(port,cert,'GET','/v1/status',timestamp=1))[0]==401,'expired proof rejected')
        wrong=dict(challenge(port,cert));wrong['challenge']='0'*64
        check(call(headers=d.headers(port,cert,'GET','/v1/status',snapshot=wrong))[0]==401,'wrong boot challenge rejected')
        for path in ['/v1/tunnel','/v1/security/ticket','/v1/security/devices','/v1/security/devices/revoke']:
            check(call(path,headers=d.headers(port,cert,'GET',path))[0]==403,'device cannot manage '+path)
        check(call('/v1/runtime/connect','POST',headers=d.headers(port,cert,'POST','/v1/runtime/connect'))[0]==200,'device retains existing runtime reconnect action')
        check(call('/v1/events',headers=d.headers(port,cert,'GET','/v1/status'))[0]==401,'WebSocket upgrade requires route-bound proof')
        ws=WebSocket(port,'',cert,proof=d.headers(port,cert,'GET','/v1/events'))
        check(ws.receive()['sequence']>=1,'signed event WebSocket receives native events')
        check(admin('/v1/security/devices/revoke','POST',{'deviceID':d.id})[0]==200,'local administrator revokes device')
        ws.socket.settimeout(4)
        try:
            while True: ws.receive()
        except (EOFError,ConnectionResetError): closed=True
        check(closed,'revocation closes existing event stream');ws.close()
        check(call(headers=d.headers(port,cert,'GET','/v1/status'))[0]==401,'revoked device cannot reconnect')
        check(call('/v1/security/pair','POST',headers=d.headers(port,cert,'POST','/v1/security/pair',ticket=ticket))[0]==403,'revocation cannot be undone by reusing consumed ticket')
        ticket=admin('/v1/security/ticket','POST',{})[1]['ticket']
        call('/v1/security/pair','POST',headers=other.headers(port,cert,'POST','/v1/security/pair',ticket=ticket))
        stale=other.headers(port,cert,'GET','/v1/status')
        process.terminate();process.wait(timeout=10);process=start();cert=ready()
        check(call(headers=stale)[0]==401,'restart rejects pre-restart proof even if never submitted')
        check(call(headers=other.headers(port,cert,'GET','/v1/status'))[0]==200,'device registration survives Host restart')
        check(call(headers=d.headers(port,cert,'GET','/v1/status'))[0]==401,'revocation survives Host restart')
        check((state/'devices-v2.json').stat().st_mode & 0o777==0o600,'registry stored with owner-only permissions')
        check(ticket not in (state/'devices-v2.json').read_text(),'pairing secret persisted only as digest')
        # Expiration and corrupt storage must fail closed on restart.
        process.terminate();process.wait(timeout=10)
        registry=json.loads((state/'devices-v2.json').read_text());registry['ticket']['expiresAt']=1
        (state/'devices-v2.json').write_text(json.dumps(registry));process=start();cert=ready()
        check(call('/v1/security/pair','POST',headers=other.headers(port,cert,'POST','/v1/security/pair',ticket=ticket))[0]==403,'expired ticket rejected')
        process.terminate();process.wait(timeout=10);(state/'devices-v2.json').write_text('broken');process=start();process.wait(timeout=10)
        check(process.returncode!=0,'corrupt device registry fails startup closed')
        print(f'PASS {checks} security checks')
    finally:
        if process.poll() is None: process.terminate();process.wait(timeout=10)
        log.close()
