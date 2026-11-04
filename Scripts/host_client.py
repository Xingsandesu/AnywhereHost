"""本机 Unix socket 管理接口。"""
import json
import socket
import struct
import uuid

class HostClient:
    def __init__(self, directory):
        self.directory = directory

    def request(self, method, body=None):
        raw = json.dumps({'id': str(uuid.uuid4()), 'method': method, 'params': body or {}}).encode()
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
            connection.settimeout(80)
            connection.connect(str(self.directory / 'control.sock'))
            connection.sendall(struct.pack('!I', len(raw)) + raw)
            def read(size):
                data = bytearray()
                while len(data) < size:
                    chunk = connection.recv(size - len(data))
                    if not chunk: raise ConnectionError('本机服务已断开')
                    data.extend(chunk)
                return data
            size = struct.unpack('!I', read(4))[0]
            if size > 20 * 1024 * 1024: raise ValueError('响应过大')
            result = json.loads(read(size))
            if isinstance(result,dict) and result.get('error') is not None: raise RuntimeError(result['error'])
            return result
