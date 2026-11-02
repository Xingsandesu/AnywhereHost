"""Signed WSS byte transport for isolated desktop regressions."""
from security_wire import admin_headers
import base64, hashlib, os, socket, ssl, struct

def exact(sock, size):
    out = b''
    while len(out) < size:
        part = sock.recv(size-len(out))
        if not part: raise EOFError()
        out += part
    return out

class DesktopSocket:
    def __init__(self, port, token, cert, origin=None):
        self.sock = ssl._create_unverified_context().wrap_socket(socket.create_connection(('127.0.0.1', port)), server_hostname='localhost')
        self.sock.settimeout(5)
        assert self.sock.getpeercert(binary_form=True) == cert
        key = base64.b64encode(os.urandom(16)).decode()
        auth = ''.join(f'{k}: {v}\r\n' for k,v in admin_headers(port,token,cert,'GET','/v1/desktop/socket').items())
        headers = f'GET /v1/desktop/socket HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n{auth}'
        if origin: headers += f'Origin: {origin}\r\n'
        self.sock.sendall((headers+'\r\n').encode())
        head = b''
        while not head.endswith(b'\r\n\r\n'): head += exact(self.sock, 1)
        self.status = int(head.split()[1])
        if self.status == 101:
            assert base64.b64encode(hashlib.sha1((key+'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()) in head
        else: self.close()
    def send(self, data, opcode=2):
        mask = os.urandom(4); n = len(data)
        head = bytes([0x80 | opcode])
        head += bytes([0x80 | n]) if n < 126 else bytes([0xfe])+struct.pack('!H', n) if n < 65536 else bytes([0xff])+struct.pack('!Q', n)
        self.sock.sendall(head+mask+bytes(b ^ mask[i%4] for i,b in enumerate(data)))
    def receive(self):
        a,b = exact(self.sock, 2); n = b & 127
        if n == 126: n = struct.unpack('!H', exact(self.sock, 2))[0]
        if n == 127: n = struct.unpack('!Q', exact(self.sock, 8))[0]
        if a & 15 == 8: raise EOFError()
        return exact(self.sock, n)
    def close(self): self.sock.close()
