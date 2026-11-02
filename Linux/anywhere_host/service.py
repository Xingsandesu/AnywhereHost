"""Own only our child process; leave external Host/Codex processes untouched."""
import fcntl
import json
import os
import signal
import subprocess
import tempfile
import time
import threading
from pathlib import Path


def private_json(path, value):
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode='w', dir=path.parent, delete=False) as output:
        name = output.name
        try:
            json.dump(value, output, indent=2)
            output.flush()
            os.fsync(output.fileno())
            os.replace(name, path)
        finally:
            if os.path.exists(name):
                os.unlink(name)


class HostService:
    def __init__(self, binary, directory):
        self.binary, self.directory = Path(binary).resolve(), Path(directory).resolve()
        self.child = None
        self.cancelled = threading.Event()

    @property
    def running(self):
        return self.child is not None and self.child.poll() is None

    def start(self):
        self.cancelled.clear()
        if self.running:
            return
        self.directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.directory.chmod(0o700)
        fd = os.open(self.directory / 'host.log', os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
        with os.fdopen(fd, 'ab') as log:
            os.fchmod(log.fileno(), 0o600)
            self.child = subprocess.Popen(
                [str(self.binary), '--state-dir', str(self.directory)], cwd=Path.home(),
                stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)

    def stop(self):
        if self.running:
            self.child.terminate()
            try:
                self.child.wait(timeout=8)
            except subprocess.TimeoutExpired:
                # All descendants in this group were created by our backend. A shared
                # Codex process is external and is never in this process group.
                os.killpg(self.child.pid, signal.SIGKILL)
                self.child.wait()
        self.child = None

    @staticmethod
    def default_codex_home():
        return os.environ.get('CODEX_HOME', str(Path.home() / '.codex'))

    def settings(self):
        path = self.directory / 'config.json'
        return json.loads(path.read_text()) if path.exists() else {
            'listen': '127.0.0.1:9443',
            'codexHome': self.default_codex_home(),
        }

    def save_settings(self, codex_home, binary, socket, port):
        if self.running:
            raise RuntimeError('请先停止服务，再修改运行时设置')
        for name, value in [('Codex Home', codex_home), ('Codex 可执行文件', binary), ('控制 socket', socket)]:
            if value and not Path(value).is_absolute():
                raise ValueError(f'{name} 必须是绝对路径')
        if not codex_home or not 1 <= int(port) <= 65535:
            raise ValueError('Codex Home 不能为空，端口范围为 1–65535')
        self.directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        fd = os.open(self.directory / 'instance.lock', os.O_RDWR | os.O_CREAT, 0o600)
        with os.fdopen(fd, 'r+') as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as error:
                raise RuntimeError('此状态目录已被外部 Host 使用，请先停止该服务') from error
            config = self.settings()
            config.update(codexHome=codex_home, codexBinary=binary or None,
                          controlSocket=socket or None, listen=f'127.0.0.1:{int(port)}')
            private_json(self.directory / 'config.json', config)

    def check_directory(self, path, cancelled=None):
        probe = self.binary.with_name('anywhere-permission-probe')
        arguments = [str(probe if probe.is_file() else self.binary), '--check-directory-access', path]
        with subprocess.Popen(arguments, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL) as child:
            deadline = time.monotonic() + 8
            try:
                while child.poll() is None:
                    if cancelled is not None and cancelled.is_set():
                        raise RuntimeError('检查已取消')
                    if time.monotonic() > deadline:
                        raise TimeoutError('检查超时')
                    time.sleep(.05)
                if child.returncode != 0:
                    raise RuntimeError('目录检查失败')
                return json.loads(child.stdout.read())
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait()

    def set_desktop(self, enabled, password=''):
        if password:
            if len(password) != 8 or not password.isascii() or any(ord(c) < 32 for c in password):
                raise ValueError('桌面密码须为 8 位 ASCII 字符（VNC 协议限制）')
            result = subprocess.run([str(self.binary.with_name('vncpasswd')), '-f'],
                input=(password + '\n').encode(), capture_output=True, check=True, timeout=5)
            if len(result.stdout) != 8:
                raise RuntimeError('桌面密码生成失败')
            path = self.directory / 'desktop.passwd'
            self.directory.mkdir(mode=0o700, parents=True, exist_ok=True)
            with tempfile.NamedTemporaryFile(dir=self.directory, delete=False) as output:
                temporary = Path(output.name)
                try:
                    output.write(result.stdout)
                    output.flush()
                    os.fsync(output.fileno())
                    os.replace(temporary, path)
                finally:
                    temporary.unlink(missing_ok=True)
        if enabled and not (self.directory / 'desktop.passwd').is_file():
            raise ValueError('请先设置独立的桌面密码')
        private_json(self.directory / 'desktop.json', {'enabled': bool(enabled)})

    def log_tail(self):
        try:
            with (self.directory / 'host.log').open('rb') as log:
                log.seek(0, os.SEEK_END)
                log.seek(max(0, log.tell() - 16384))
                return log.read().decode(errors='replace')
        except FileNotFoundError:
            return '暂无日志'
