"""Protocol v2 test/diagnostic client. Pin custom/self-signed certificates before signing."""
import base64, hashlib, hmac, http.client, json, os, ssl, subprocess

def connection(port, certificate, host='127.0.0.1'):
    context=ssl._create_unverified_context(); context.minimum_version=ssl.TLSVersion.TLSv1_3
    c=http.client.HTTPSConnection(host,port,context=context,timeout=15); c.connect()
    assert c.sock.getpeercert(binary_form=True)==certificate, 'Host certificate mismatch'
    return c

def challenge(port, certificate):
    c=connection(port,certificate)
    try:
        c.request('GET','/v1/security/challenge'); r=c.getresponse(); data=json.loads(r.read()); assert r.status==200
        return data
    finally: c.close()

def proof_headers(port, certificate, identity, signer, method, path, body=None, *, snapshot=None, nonce=None, timestamp=None):
    snapshot=snapshot or challenge(port,certificate)
    timestamp=str(timestamp if timestamp is not None else snapshot['serverTime'])
    nonce=nonce or os.urandom(16).hex()
    raw=body.encode() if isinstance(body,str) else body or b''
    digest=hashlib.sha256(raw).hexdigest()
    canonical='\n'.join(['Anywhere-v2',hashlib.sha256(certificate).hexdigest(),identity,method,path,digest,timestamp,nonce,snapshot['challenge']]).encode()
    return {'Authorization':identity,'X-Anywhere-Challenge':snapshot['challenge'],'X-Anywhere-Time':timestamp,'X-Anywhere-Nonce':nonce,
            'X-Anywhere-Content-SHA256':digest,'X-Anywhere-Signature':base64.b64encode(signer(canonical)).decode()}

def admin_headers(port, token, certificate, method, path, body=None):
    if not token: return {}
    return proof_headers(port,certificate,'Anywhere-Admin',lambda data:hmac.digest(token.encode(),data,'sha256'),method,path,body)

class Device:
    def __init__(self, directory):
        self.key=directory/'device.pem'
        subprocess.run(['openssl','genpkey','-algorithm','EC','-pkeyopt','ec_paramgen_curve:P-256','-out',str(self.key)],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        self.key.chmod(0o600)
        self.public=subprocess.check_output(['openssl','pkey','-in',str(self.key),'-pubout','-outform','DER'])[-65:]
        self.id=hashlib.sha256(self.public).hexdigest()
    def sign(self, data):
        der=subprocess.run(['openssl','dgst','-sha256','-sign',str(self.key)],input=data,check=True,stdout=subprocess.PIPE).stdout
        assert der[0]==48 and der[2]==2
        rsize=der[3]; r=int.from_bytes(der[4:4+rsize]); pos=4+rsize
        assert der[pos]==2
        ssize=der[pos+1]; s=int.from_bytes(der[pos+2:pos+2+ssize])
        return r.to_bytes(32,'big')+s.to_bytes(32,'big')
    def headers(self, port, certificate, method, path, body=None, ticket=None, **kw):
        identity='Anywhere-Pair '+ticket if ticket else 'Anywhere-Device '+self.id
        headers=proof_headers(port,certificate,identity,self.sign,method,path,body,**kw)
        if ticket: headers['X-Anywhere-Public-Key']=base64.b64encode(self.public).decode()
        return headers
