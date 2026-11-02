#!/usr/bin/env python3
"""Fetch pinned upstream frpc; retain its license in the platform package."""
import argparse
import hashlib
import io
import pathlib
import platform
import tarfile
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[1]
VERSION = '0.71.1'
DIGESTS = {
    ('darwin', 'arm64'): 'd15d0324b99e14619c9aa46e37f6db588c06a09be4bc6f7948890b6b61077842',
    ('darwin', 'amd64'): '8acb59b6fb998cc8d209d1373d90bf70fa5ec3efeb4dfbc7bda6ce3abd78cb45',
    ('linux', 'amd64'): '1d9875210972a5d1a62b2dc4c48df45ab0af05d3ca1b5e494a014428259df803',
    ('linux', 'arm64'): '8edf590b0c396fa42f9c147121a76af83cfc2002b08f5aef0ea1008c43482019',
}
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output-dir', type=pathlib.Path, default=ROOT / 'Resources')
args = parser.parse_args()
system = platform.system().lower()
arch = {'arm64': 'arm64', 'aarch64': 'arm64', 'x86_64': 'amd64'}.get(platform.machine(), platform.machine())
if (system, arch) not in DIGESTS:
    raise SystemExit(f'Unsupported frpc target: {system}/{arch}')
digest = DIGESTS[(system, arch)]
name = f'frp_{VERSION}_{system}_{arch}'
url = f'https://github.com/fatedier/frp/releases/download/v{VERSION}/{name}.tar.gz'
cache = ROOT / '.build' / f'{name}.tar.gz'
cache.parent.mkdir(parents=True, exist_ok=True)
if not cache.exists() or hashlib.sha256(cache.read_bytes()).hexdigest() != digest:
    subprocess.run(['/usr/bin/curl', '--http1.1', '--fail', '--location', '--silent', '--show-error', '--retry', '3', '--connect-timeout', '20', '--max-time', '120', url, '-o', str(cache)], check=True)
archive = cache.read_bytes()
if hashlib.sha256(archive).hexdigest() != digest:
    raise SystemExit('frp archive checksum mismatch')
with tarfile.open(fileobj=io.BytesIO(archive), mode='r:gz') as tar:
    for source, destination, mode in [('frpc', 'frpc', 0o755), ('LICENSE', 'frp-LICENSE', 0o644)]:
        member = tar.extractfile(f'{name}/{source}')
        if member is None:
            raise SystemExit(f'Missing {source}')
        args.output_dir.mkdir(parents=True, exist_ok=True)
        path = args.output_dir / destination
        path.write_bytes(member.read())
        path.chmod(mode)
print(f'Verified frpc {VERSION} ({system}/{arch})')
