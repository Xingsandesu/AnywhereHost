#!/usr/bin/env python3
"""Build checksum-pinned TigerVNC desktop helpers; include corresponding source."""
import argparse
import hashlib
import os
from pathlib import Path
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parents[1]
VERSION = '1.16.2'
DIGEST = 'b107c0c8b8a962594281690366c24186e95c2ea4a169acbc0076aa62ed01f467'


def build(container=False):
    cache = ROOT / '.build'
    cache.mkdir(exist_ok=True)
    archive = cache / f'tigervnc-{VERSION}.tar.gz'
    if not archive.exists() or hashlib.sha256(archive.read_bytes()).hexdigest() != DIGEST:
        subprocess.run(['curl', '--fail', '--location', '--retry', '3',
                        f'https://codeload.github.com/TigerVNC/tigervnc/tar.gz/refs/tags/v{VERSION}',
                        '-o', str(archive)], check=True)
    if hashlib.sha256(archive.read_bytes()).hexdigest() != DIGEST:
        raise RuntimeError('TigerVNC source checksum mismatch')
    source = cache / f'tigervnc-{VERSION}'
    if not source.exists():
        with tarfile.open(archive) as tar:
            tar.extractall(cache, filter='data')
    output = source / 'build'
    marker = output / 'anywhere-source.sha256'
    helpers = [output / 'unix' / name / name for name in ('w0vncserver', 'x0vncserver', 'vncpasswd')]
    if marker.exists() and marker.read_text() == DIGEST and all(path.is_file() for path in helpers):
        return source
    if container:
        subprocess.run(['docker', 'build', '-f', str(ROOT / 'Linux/packaging/Dockerfile.desktop'),
                        '-t', 'anywhere-host-desktop-builder', str(ROOT / 'Linux/packaging')], check=True)
        prefix = ['docker', 'run', '--rm', '--user', f'{os.getuid()}:{os.getgid()}',
                  '-v', f'{ROOT}:{ROOT}', '-w', str(ROOT), 'anywhere-host-desktop-builder']
    else:
        prefix = []
    subprocess.run(prefix + ['cmake', '-S', str(source), '-B', str(output), '-DBUILD_VIEWER=OFF',
                            '-DENABLE_WAYLAND=ON', '-DENABLE_NLS=OFF', '-DENABLE_H264=OFF',
                            '-DENABLE_NETTLE=OFF', '-DENABLE_PWQUALITY=OFF', '-DENABLE_SYSTEMD=OFF',
                            '-DENABLE_SELINUX=OFF', '-DBUILD_TIMESTAMP=2026-03-26',
                            f'-DCMAKE_C_FLAGS=-ffile-prefix-map={ROOT}=/usr/src/anywhere-host',
                            f'-DCMAKE_CXX_FLAGS=-ffile-prefix-map={ROOT}=/usr/src/anywhere-host'], check=True)
    subprocess.run(prefix + ['cmake', '--build', str(output), '--target', 'w0vncserver', 'x0vncserver',
                            'vncpasswd', '-j', str(min(os.cpu_count() or 2, 8))], check=True)
    marker.write_text(DIGEST)
    return source


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--container', action='store_true')
    args = parser.parse_args()
    print(build(args.container))
