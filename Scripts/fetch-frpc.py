#!/usr/bin/env python3
"""Fetch pinned upstream frpc; retain its license in the .app bundle."""
import hashlib
import io
import pathlib
import platform
import tarfile
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[1]
VERSION = '0.71.1'
DIGESTS = {
    'arm64': 'd15d0324b99e14619c9aa46e37f6db588c06a09be4bc6f7948890b6b61077842',
    'amd64': '8acb59b6fb998cc8d209d1373d90bf70fa5ec3efeb4dfbc7bda6ce3abd78cb45',
}
arch = {'arm64': 'arm64', 'x86_64': 'amd64'}[platform.machine()]
name = f'frp_{VERSION}_darwin_{arch}'
url = f'https://github.com/fatedier/frp/releases/download/v{VERSION}/{name}.tar.gz'
cache = ROOT / '.build' / f'{name}.tar.gz'
cache.parent.mkdir(parents=True, exist_ok=True)
if not cache.exists() or hashlib.sha256(cache.read_bytes()).hexdigest() != DIGESTS[arch]:
    subprocess.run(['/usr/bin/curl', '--http1.1', '--fail', '--location', '--silent', '--show-error', '--retry', '3', '--connect-timeout', '20', '--max-time', '120', url, '-o', str(cache)], check=True)
archive = cache.read_bytes()
if hashlib.sha256(archive).hexdigest() != DIGESTS[arch]:
    raise SystemExit('frp archive checksum mismatch')
with tarfile.open(fileobj=io.BytesIO(archive), mode='r:gz') as tar:
    for source, destination, mode in [('frpc', 'frpc', 0o755), ('LICENSE', 'frp-LICENSE', 0o644)]:
        member = tar.extractfile(f'{name}/{source}')
        if member is None:
            raise SystemExit(f'Missing {source}')
        path = ROOT / 'Resources' / destination
        path.write_bytes(member.read())
        path.chmod(mode)
print(f'Verified frpc {VERSION} ({arch})')
