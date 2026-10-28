import base64, hashlib, json, os, socket, ssl, struct

class WebSocket:
    def __init__(self, port, token, certificate, query=''):
        self.socket = ssl._create_unverified_context().wrap_socket(socket.create_connection(('127.0.0.1', port)), server_hostname='localhost')
        self.socket.settimeout(5)
        assert self.socket.getpeercert(binary_form=True) == certificate
        key = base64.b64encode(os.urandom(16)).decode()
        request = f'GET /v1/events{query} HTTP/1.1\r\nHost: localhost:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nAuthorization: Bearer {token}\r\n\r\n'
        self.socket.sendall(request.encode())
        header = b''
        while not header.endswith(b'\r\n\r\n'): header += self.socket.recv(1)
        assert b'101 Switching Protocols' in header, header.split(b'\r\n')[0]
        accept = base64.b64encode(hashlib.sha1((key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest())
        assert accept in header
    def read_exact(self, size):
        result = b''
        while len(result) < size:
            chunk = self.socket.recv(size - len(result))
            if not chunk: raise EOFError()
            result += chunk
        return result
    def receive(self):
        while True:
            a, b = self.read_exact(2); size = b & 127
            if size == 126: size = struct.unpack('!H', self.read_exact(2))[0]
            if size == 127: size = struct.unpack('!Q', self.read_exact(8))[0]
            payload = self.read_exact(size)
            if a & 15 == 1: return json.loads(payload)
            if a & 15 == 8: raise EOFError()
    def until(self, method):
        for _ in range(20):
            event = self.receive()
            if event.get('payload', {}).get('method') == method: return event
        raise AssertionError(method)
    def close(self): self.socket.close()
