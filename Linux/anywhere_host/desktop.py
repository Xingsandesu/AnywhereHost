"""Locally request screen/input consent by authenticating our private RFB helper."""
import socket
import struct
from cryptography.hazmat.primitives.ciphers import Cipher, modes
from cryptography.hazmat.decrepit.ciphers.algorithms import TripleDES
import time


def authorize(directory, password, cancelled):
    if len(password) != 8 or not password.isascii():
        raise ValueError('请填写独立桌面密码后进行授权检查')
    deadline = time.monotonic() + 90
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(1)
        connection.connect(str(directory / 'desktop.sock'))
        def read(count):
            data = b''
            while len(data) < count:
                if cancelled.is_set():
                    raise RuntimeError('授权检查已取消')
                if time.monotonic() >= deadline:
                    raise TimeoutError('屏幕授权超时，请在本机系统弹窗中选择屏幕并允许输入')
                try:
                    part = connection.recv(count - len(data))
                except socket.timeout:
                    continue
                if not part:
                    raise RuntimeError('屏幕授权被取消或共享进程已断开')
                data += part
            return data
        if read(12) != b'RFB 003.008\n':
            raise RuntimeError('桌面协议版本不受支持')
        connection.sendall(b'RFB 003.008\n')
        if 2 not in read(read(1)[0]):
            raise RuntimeError('桌面服务没有提供独立密码认证')
        connection.sendall(b'\x02')
        key = bytes(int(f'{byte:08b}'[::-1], 2) for byte in password.encode())
        cipher = Cipher(TripleDES(key), modes.ECB()).encryptor()
        proof = cipher.update(read(16)) + cipher.finalize()
        connection.sendall(proof)
        if read(4) != b'\0' * 4:
            raise ValueError('桌面密码错误')
        connection.sendall(b'\x01')
        width, height = struct.unpack('!HH', read(4))
        if not width or not height:
            raise RuntimeError('桌面尚未提供有效画面')
        read(16)
        length = struct.unpack('!I', read(4))[0]
        if length > 65536:
            raise RuntimeError('桌面响应无效')
        read(length)
        return f'屏幕会话已授权（{width} × {height}）；输入控制以本机授权选择为准'
