"""Signed Debian updates; the privilege helper re-verifies a private copy."""
import base64
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import tempfile
import urllib.request
from urllib.parse import urlsplit

REPOSITORY = 'https://github.com/Xingsandesu/AnywhereHost'
API = 'https://api.github.com/repos/Xingsandesu/AnywhereHost/releases/latest'
MAX_PACKAGE = 256 * 1024 * 1024


def native_architecture():
    architecture = subprocess.check_output(['dpkg', '--print-architecture'], text=True, timeout=10).strip()
    if architecture not in ('amd64', 'arm64'):
        raise ValueError(f'不支持的 Debian 架构：{architecture}')
    return architecture


def public_key():
    bundled = Path(__file__).resolve().parent.parent / 'update-public-key.pem'
    return bundled if bundled.exists() else Path(__file__).resolve().parents[1] / 'packaging/update-public-key.pem'


def verify(package, signature, key=None):
    raw = base64.b64decode(Path(signature).read_text().strip(), validate=True)
    if len(raw) != 64:
        raise ValueError('更新签名长度无效')
    with tempfile.NamedTemporaryFile() as temporary:
        temporary.write(raw)
        temporary.flush()
        result = subprocess.run(['openssl', 'pkeyutl', '-verify', '-pubin', '-inkey', str(key or public_key()),
                                 '-rawin', '-in', str(package), '-sigfile', temporary.name],
                                capture_output=True, timeout=60)
    if result.returncode:
        raise ValueError('更新包 Ed25519 签名验证失败')


def newer(candidate, installed):
    return subprocess.run(['dpkg', '--compare-versions', candidate, 'gt', installed],
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0


def metadata(package):
    values = subprocess.check_output(['dpkg-deb', '-f', str(package), 'Package', 'Version', 'Architecture'],
                                     text=True, timeout=10)
    return dict(line.split(': ', 1) for line in values.splitlines())


class HTTPSRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, url):
        parsed = urlsplit(url)
        if parsed.scheme != 'https' or parsed.hostname not in ('github.com', 'release-assets.githubusercontent.com', 'objects.githubusercontent.com'):
            raise ValueError('更新下载重定向地址不受信任')
        return super().redirect_request(request, response, code, message, headers, url)


def download(url, destination, limit):
    request = urllib.request.Request(url, headers={'User-Agent': 'AnywhereHost-Linux', 'Accept': 'application/octet-stream'})
    opener = urllib.request.build_opener(HTTPSRedirect())
    with opener.open(request, timeout=30) as response, open(destination, 'wb') as output:
        count = 0
        while data := response.read(1024 * 1024):
            count += len(data)
            if count > limit:
                raise ValueError('更新下载超过大小限制')
            output.write(data)


class Updater:
    def __init__(self, service):
        self.service = service
        self.candidate = None
        self.downloaded = None
        self.directory = None

    def current_version(self):
        result = subprocess.run(['dpkg-query', '-W', '-f=${Version}', 'anywhere-host'], capture_output=True, text=True)
        if result.returncode == 0:
            return result.stdout.strip()
        return subprocess.check_output([str(self.service.binary), '--version'], text=True).split()[-1] + '-1'

    def check(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'release.json'
            download(API, path, 2 * 1024 * 1024)
            release = json.loads(path.read_text())
        current = self.current_version()
        architecture = native_architecture()
        assets = {asset['name']: asset for asset in release.get('assets', [])}
        self.candidate = None
        for name, asset in assets.items():
            match = re.fullmatch(rf'anywhere-host_(\d+\.\d+\.\d+-\d+)_{architecture}\.deb', name)
            if not match or name + '.sig' not in assets:
                continue
            for selected in (asset, assets[name + '.sig']):
                url = selected['browser_download_url']
                if not url.startswith(REPOSITORY + '/releases/download/') or urlsplit(url).query:
                    raise ValueError('更新发布地址不受信任')
            if newer(match[1], current):
                self.candidate = {'version': match[1], 'name': name, 'url': asset['browser_download_url'],
                                  'signature': assets[name + '.sig']['browser_download_url']}
                return f'发现新版本 {match[1]}（当前 {current}）'
        return f'当前 {current}；尚无更新的已签名 Debian {architecture} 发行包'

    def fetch(self):
        if not self.candidate:
            raise ValueError('请先检查更新')
        self.close()
        self.directory = tempfile.TemporaryDirectory(prefix='anywhere-update-')
        package = Path(self.directory.name) / self.candidate['name']
        signature = package.with_suffix('.deb.sig')
        download(self.candidate['url'], package, MAX_PACKAGE)
        download(self.candidate['signature'], signature, 1024)
        verify(package, signature)
        info = metadata(package)
        if info != {'Package': 'anywhere-host', 'Version': self.candidate['version'], 'Architecture': native_architecture()}:
            raise ValueError('更新包身份与发布信息不一致')
        self.downloaded = package
        return f"{info['Version']} 已下载并通过 Ed25519 验证"

    def install(self):
        if not self.downloaded:
            raise ValueError('请先下载并验证更新')
        result = subprocess.run(['pkexec', '/usr/lib/anywhere-host/install-update', str(self.downloaded),
                                 str(self.downloaded.with_suffix('.deb.sig'))], capture_output=True, text=True)
        if result.returncode:
            raise RuntimeError('安装取消或失败：' + (result.stderr[-2000:] or result.stdout[-2000:]))

    def close(self):
        self.downloaded = None
        if self.directory:
            self.directory.cleanup()
            self.directory = None


def private_copy(source, destination, limit):
    descriptor = os.open(source, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, 'rb') as incoming, open(destination, 'xb') as outgoing:
        info = os.fstat(incoming.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size > limit:
            raise ValueError('更新输入文件无效')
        count = 0
        while data := incoming.read(1024 * 1024):
            count += len(data)
            if count > limit:
                raise ValueError('更新输入过大')
            outgoing.write(data)


def install_verified(package, signature):
    if os.getuid() != 0:
        raise PermissionError('更新安装需要系统授权')
    with tempfile.TemporaryDirectory(prefix='anywhere-install-') as directory:
        safe_package, safe_signature = Path(directory) / 'update.deb', Path(directory) / 'update.sig'
        private_copy(package, safe_package, MAX_PACKAGE)
        private_copy(signature, safe_signature, 1024)
        verify(safe_package, safe_signature)
        info = metadata(safe_package)
        installed = subprocess.check_output(['dpkg-query', '-W', '-f=${Version}', 'anywhere-host'], text=True).strip()
        if info.get('Package') != 'anywhere-host' or info.get('Architecture') != native_architecture() or not newer(info.get('Version', ''), installed):
            raise ValueError('更新身份无效或版本不高于当前安装版本')
        subprocess.run(['/usr/bin/apt-get', 'install', '--yes', '--no-remove', str(safe_package)], check=True,
                       env={'PATH': '/usr/sbin:/usr/bin:/sbin:/bin', 'DEBIAN_FRONTEND': 'noninteractive'})
