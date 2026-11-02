#!/usr/bin/env python3
"""Real TigerVNC capture + input over signed Host WSS, exclusively on isolated Xvfb."""
import ctypes
import os
from pathlib import Path
import socket
import struct
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, os.environ.get('ANYWHERE_TEST_UI', str(ROOT / 'Linux')))
from anywhere_host.client import HostClient
from anywhere_host.service import HostService, private_json
from anywhere_host.desktop import authorize
from desktop_wire import DesktopSocket

if os.environ.get('ANYWHERE_ISOLATED_DISPLAY') != '1' or os.environ.get('WAYLAND_DISPLAY'):
    raise SystemExit('Run only under an isolated Xvfb with ANYWHERE_ISOLATED_DISPLAY=1 and no WAYLAND_DISPLAY')


class RFB:
    def __init__(self, connection):
        self.connection, self.buffer = connection, b''

    def read(self, count):
        while len(self.buffer) < count:
            self.buffer += self.connection.receive()
        value, self.buffer = self.buffer[:count], self.buffer[count:]
        return value

    def authenticate(self, password):
        assert self.read(12) == b'RFB 003.008\n'
        self.connection.send(b'RFB 003.008\n')
        methods = self.read(self.read(1)[0])
        assert 2 in methods and 1 not in methods, 'Independent VNC authentication required'
        self.connection.send(b'\x02')
        key = bytes(int(f'{byte:08b}'[::-1], 2) for byte in password.encode()).hex()
        response = subprocess.run(['openssl', 'enc', '-des-ecb', '-provider', 'legacy', '-provider', 'default',
                                   '-nopad', '-nosalt', '-K', key], input=self.read(16), capture_output=True, check=True).stdout
        self.connection.send(response)
        assert self.read(4) == b'\0' * 4
        self.connection.send(b'\x01')
        width, height = struct.unpack('!HH', self.read(4))
        self.read(16)
        self.read(struct.unpack('!I', self.read(4))[0])
        return width, height


with tempfile.TemporaryDirectory(prefix='anywhere-real-rfb-') as directory:
    state = Path(directory)
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        port = listener.getsockname()[1]
    private_json(state / 'config.json', {'listen': f'127.0.0.1:{port}', 'codexHome': directory,
                 'codexBinary': str(ROOT / 'Scripts/fake-codex.py')})
    service = HostService(os.environ.get('ANYWHERE_TEST_BINARY', ROOT / 'target/debug/anywhere-host'), state)
    service.set_desktop(True, 'test1234')
    service.start()
    connections = []
    try:
        for _ in range(200):
            try:
                client = HostClient(state)
                if client.request('/v1/desktop')['reachable']:
                    break
            except (OSError, ValueError):
                pass
            time.sleep(.05)
        else:
            raise AssertionError(service.log_tail() + (state / 'desktop.log').read_text())
        assert '已授权' in authorize(state, 'test1234', service.cancelled)
        service.save_desktop_password('new12345')
        try:
            authorize(state, 'test1234', service.cancelled)
        except ValueError as error:
            assert '密码错误' in str(error)
        else:
            raise AssertionError('Old password still accepted after saving the new password')
        assert '已授权' in authorize(state, 'new12345', service.cancelled)
        assert (state / 'desktop.sock').stat().st_mode & 0o777 == 0o600
        a = DesktopSocket(port, client.key.decode(), client.certificate)
        connections.append(a)
        assert a.status == 101
        rfb = RFB(a)
        width, height = rfb.authenticate('new12345')
        assert width > 0 and height > 0
        # Request an actual framebuffer rectangle, using the portable raw encoding.
        a.send(b'\x02\0\0\x01' + struct.pack('!i', 0))
        a.send(b'\x03\0' + struct.pack('!HHHH', 0, 0, 16, 16))
        assert rfb.read(1) == b'\0'
        rfb.read(1)
        count = struct.unpack('!H', rfb.read(2))[0]
        assert count > 0
        pixel_count = 0
        for _ in range(count):
            x, y, w, h, encoding = struct.unpack('!HHHHi', rfb.read(12))
            assert encoding == 0
            pixels = rfb.read(w * h * 4)
            pixel_count += len(pixels)
        assert pixel_count > 0
        # Read actual pointer and keyboard state from this isolated X server.
        xlib = ctypes.CDLL('libX11.so.6')
        xlib.XOpenDisplay.argtypes = [ctypes.c_char_p]; xlib.XOpenDisplay.restype = ctypes.c_void_p
        display = xlib.XOpenDisplay(None)
        assert display
        xlib.XDefaultRootWindow.argtypes = [ctypes.c_void_p]; xlib.XDefaultRootWindow.restype = ctypes.c_ulong
        root = xlib.XDefaultRootWindow(display)
        xlib.XQueryPointer.argtypes = [ctypes.c_void_p, ctypes.c_ulong] + [ctypes.c_void_p] * 7
        xlib.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]; xlib.XKeysymToKeycode.restype = ctypes.c_ubyte
        xlib.XQueryKeymap.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
        a.send(b'\x05\0' + struct.pack('!HH', 40, 60))
        a.send(b'\x04\x01\0\0' + struct.pack('!I', ord('a')))
        time.sleep(.15)
        root_ret, child_ret = ctypes.c_ulong(), ctypes.c_ulong()
        rx, ry, wx, wy, mask = [ctypes.c_int() for _ in range(5)]
        xlib.XQueryPointer(display, root, *[ctypes.byref(v) for v in (root_ret, child_ret, rx, ry, wx, wy, mask)])
        assert (rx.value, ry.value) == (40, 60)
        keys = ctypes.create_string_buffer(32)
        xlib.XQueryKeymap(display, keys)
        code = xlib.XKeysymToKeycode(display, ord('a'))
        assert keys.raw[code // 8] & (1 << (code % 8))
        a.send(b'\x04\0\0\0' + struct.pack('!I', ord('a')))
        b = DesktopSocket(port, client.key.decode(), client.certificate)
        connections.append(b)
        assert b.status == 101
        assert DesktopSocket(port, client.key.decode(), client.certificate).status == 429
        service.set_desktop(False)
        for _ in range(40):
            if not client.request('/v1/desktop')['enabled']:
                break
            time.sleep(.05)
        time.sleep(1.2)
        assert not client.request('/v1/desktop')['reachable']
        assert not (state / 'desktop.sock').exists()
        print('PASS actual Linux framebuffer, keyboard/pointer input, password replacement and old-password rejection, private socket, two-session limit and local revocation over signed WSS')
    finally:
        for connection in connections:
            connection.close()
        service.stop()
