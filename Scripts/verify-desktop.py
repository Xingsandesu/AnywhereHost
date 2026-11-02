#!/usr/bin/env python3
from security_wire import admin_headers
"""Exercise actual TLS/WSS desktop relay with a disposable local RFB byte peer.
Requires unused loopback port 5900; never changes macOS screen sharing settings.
Use --serve to keep a small framebuffer fixture for Simulator verification.
"""
import argparse, base64, hashlib, http.client, json, os, pathlib, socket, ssl, struct, subprocess, tempfile, threading, time
ROOT = pathlib.Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--serve', action='store_true')
parser.add_argument('--binary', type=pathlib.Path, default=ROOT/'target/debug/anywhere-host')
args = parser.parse_args()

from desktop_wire import DesktopSocket, exact

listener = socket.socket(); listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
try: listener.bind(('127.0.0.1', 5900))
except OSError: raise SystemExit('Port 5900 is in use; fixture refuses to replace the existing screen sharing service.')
listener.listen()
def peer(client):
    try:
        client.settimeout(30); client.sendall(b'RFB 003.008\n')
        if not args.serve:
            while True:
                data = client.recv(65536)
                if not data: break
                client.sendall(data)
            return
        exact(client, 12); client.sendall(b'\x01\x01'); exact(client, 1)
        client.sendall(b'\0'*4); exact(client, 1)
        width, height = 640, 400
        name = b'Anywhere Desktop Fixture'
        client.sendall(struct.pack('!HH', width,height)+bytes([32,24,0,1,0,255,0,255,0,255,16,8,0,0,0,0])+struct.pack('!I',len(name))+name)
        pixels = b''.join(bytes((80 if x<320 else 180, 170 if y<200 else 70, 40,0)) for y in range(height) for x in range(width))
        while True:
            kind = exact(client,1)[0]
            if kind == 0: exact(client,19)
            elif kind == 2:
                header = exact(client,3); exact(client,struct.unpack('!H',header[1:])[0]*4)
            elif kind == 3:
                exact(client,9)
                client.sendall(b'\0\0\0\x01'+struct.pack('!HHHHi',0,0,width,height,0)+pixels)
                time.sleep(0.04)
            elif kind == 4: exact(client,7)
            elif kind == 5: exact(client,5)
            elif kind == 6:
                header=exact(client,7);exact(client,struct.unpack('!I',header[3:])[0])
            else: break
    except (OSError,EOFError): pass
    finally: client.close()
def accept():
    while True:
        try: client,_=listener.accept()
        except OSError: break
        threading.Thread(target=peer,args=(client,),daemon=True).start()
threading.Thread(target=accept,daemon=True).start()
with tempfile.TemporaryDirectory(prefix='anywhere-desktop-') as directory:
    state=pathlib.Path(directory); home=state/'codex';home.mkdir()
    with socket.socket() as reserve: reserve.bind(('127.0.0.1',0));port=reserve.getsockname()[1]
    (state/'config.json').write_text(json.dumps({'listen':f'127.0.0.1:{port}','codexHome':str(home),'codexBinary':str(ROOT/'Scripts/fake-codex.py')}))
    def setting(enabled):
        temporary=state/'desktop.tmp';temporary.write_text(json.dumps({'enabled':enabled}));temporary.replace(state/'desktop.json')
    def call(path, token):
        c=http.client.HTTPSConnection('127.0.0.1',port,context=ssl._create_unverified_context(),timeout=5)
        c.request('GET',path,headers=admin_headers(port,token,ssl.PEM_cert_to_DER_cert((state/'host-cert.pem').read_text()),'GET',path));r=c.getresponse();value=(r.status,json.loads(r.read()));c.close();return value
    with (state/'log').open('w') as log:
        process=subprocess.Popen([str(args.binary.resolve()),'--state-dir',str(state)],stdout=log,stderr=log)
        try:
            for _ in range(100):
                try:
                    token=(state/'admin-key-v2').read_text()
                    if call('/v1/status',token)[0]==200: break
                except (OSError,ValueError): pass
                time.sleep(.1)
            else: raise RuntimeError('Host did not start')
            cert=ssl.PEM_cert_to_DER_cert((state/'host-cert.pem').read_text())
            assert call('/v1/desktop','')[0]==401
            assert not call('/v1/desktop',token)[1]['enabled']
            assert DesktopSocket(port,token,cert).status==403
            setting(True)
            assert call('/v1/desktop',token)[1]['reachable']
            assert DesktopSocket(port,'wrong',cert).status==401
            assert DesktopSocket(port,token,cert,'https://hostile.invalid').status==403
            if args.serve:
                import urllib.parse
                c=http.client.HTTPSConnection('127.0.0.1',port,context=ssl._create_unverified_context()); c.request('POST','/v1/security/ticket',headers=admin_headers(port,token,cert,'POST','/v1/security/ticket'));ticket=json.loads(c.getresponse().read())['ticket'];c.close()
                link='anywhere-host://connect?'+urllib.parse.urlencode({'v':'2','endpoint':f'https://127.0.0.1:{port}','ticket':ticket,'certificateSHA256':hashlib.sha256(cert).hexdigest(),'name':'Desktop Fixture'})
                linkfile=pathlib.Path('/tmp/anywhere-desktop-fixture-link');linkfile.write_text(link);linkfile.chmod(0o600)
                print('Fixture ready. Pairing link stored privately in /tmp/anywhere-desktop-fixture-link',flush=True)
                while True: time.sleep(1)
            a=DesktopSocket(port,token,cert);b=DesktopSocket(port,token,cert)
            assert a.status==101 and b.status==101
            assert DesktopSocket(port,token,cert).status==429
            assert a.receive()==b'RFB 003.008\n'
            assert b.receive()==b'RFB 003.008\n'
            data=bytes(range(256))*2048
            a.send(data); result=b''
            while len(result)<len(data): result+=a.receive()
            assert result==data
            a.send(b'ping',9); assert a.receive()==b'ping'
            setting(False)
            for connection in (a,b):
                try: connection.receive()
                except (EOFError,ConnectionResetError,ssl.SSLError): pass
                else: raise AssertionError('revoked desktop remains open')
                connection.close()
            setting(True)
            time.sleep(.2)
            c=DesktopSocket(port,token,cert); assert c.status==101;c.receive()
            c.send(b'not RFB',1)
            try: c.receive()
            except (EOFError,ConnectionResetError,ssl.SSLError): pass
            else: raise AssertionError('text frame accepted')
            c.close()
            print('PASS: TLS certificate, auth, origin, disabled state, local reachability, two-session limit, 512 KiB binary roundtrip, ping/pong, revocation, permit release, text rejection')
        finally:
            process.terminate()
            try: process.wait(timeout=8)
            except subprocess.TimeoutExpired: process.kill();process.wait()
            listener.close()
            if args.serve: pathlib.Path('/tmp/anywhere-desktop-fixture-link').unlink(missing_ok=True)
