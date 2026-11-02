#!/usr/bin/env python3
"""Inspect and smoke an extracted deb without installing over the user's system."""
import argparse
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('package', type=Path)
parser.add_argument('--desktop', action='store_true', help='Test real desktop capture/input; requires isolated Xvfb')
parser.add_argument('--ui', action='store_true', help='Also run GTK smoke (requires display, e.g. xvfb-run)')
args = parser.parse_args()
package = args.package.resolve()
architecture = subprocess.check_output(['dpkg', '--print-architecture'], text=True).strip()
assert architecture in ('amd64', 'arm64'), f'Unsupported native architecture: {architecture}'
assert subprocess.check_output(['dpkg-deb', '-f', str(package), 'Architecture'], text=True).strip() == architecture
with tempfile.TemporaryDirectory(prefix='anywhere-deb-check-') as temporary:
    directory = Path(temporary)
    archive = directory / 'data.tar'
    with archive.open('wb') as output:
        subprocess.run(['dpkg-deb', '--fsys-tarfile', str(package)], stdout=output, check=True)
    with tarfile.open(archive) as tar:
        for member in tar:
            assert member.uid == member.gid == 0, member.name
            assert not member.mode & 0o022, member.name
            assert '__pycache__' not in member.name and not member.name.endswith('.pyc'), member.name
            assert not member.name.startswith(('./home/', './root/')), member.name
    subprocess.run(['dpkg-deb', '-x', str(package), str(directory)], check=True)
    lib = directory / 'usr/lib/anywhere-host'
    for binary in ('anywhere-host', 'anywhere-permission-probe', 'frpc', 'w0vncserver', 'x0vncserver', 'vncpasswd', 'install-update'):
        assert os.access(lib / binary, os.X_OK), binary
    # Check actual ELF headers as well as package metadata; mislabeled binaries
    # can otherwise be hidden by binfmt emulation on a build machine.
    for binary in ('anywhere-host', 'anywhere-permission-probe', 'frpc', 'w0vncserver', 'x0vncserver', 'vncpasswd'):
        with (lib / binary).open('rb') as executable:
            header = executable.read(20)
        assert header[:6] == b'\x7fELF\x02\x01', f'{binary}: expected 64-bit little-endian ELF'
        assert int.from_bytes(header[18:20], 'little') == {'amd64': 62, 'arm64': 183}[architecture], f'{binary}: wrong ELF architecture'
    for binary in ('anywhere-host', 'anywhere-permission-probe', 'w0vncserver', 'x0vncserver', 'vncpasswd'):
        assert str(Path.home()).encode() not in (lib / binary).read_bytes(), 'Builder Home leaked into ELF'
    assert (directory / 'usr/share/doc/anywhere-host/frp-LICENSE').is_file()
    assert (directory / 'usr/share/applications/com.jintongshu.AnywhereHost.desktop').is_file()
    assert not list(lib.glob('*codex*')), 'Codex must not be redistributed'
    subprocess.run([str(lib / 'anywhere-host'), '--version'], check=True)
    subprocess.run([str(lib / 'frpc'), '--version'], check=True)
    for name in ('w0vncserver', 'x0vncserver'):
        subprocess.run([str(lib / name), '-version'], check=True, capture_output=True)
    assert (directory / 'usr/share/doc/anywhere-host/tigervnc-source.tar.gz').is_file()
    env = {**os.environ, 'ANYWHERE_TEST_BINARY': str(lib / 'anywhere-host'), 'ANYWHERE_TEST_UI': str(lib),
           'PYTHONDONTWRITEBYTECODE': '1'}
    subprocess.run(['python3', '-m', 'unittest', 'discover', '-s', 'Linux/tests', '-v'], cwd=ROOT, env=env, check=True)
    subprocess.run(['python3', 'Scripts/verify-host.py', '--binary', str(lib / 'anywhere-host')], cwd=ROOT, check=True)
    if args.ui:
        subprocess.run(['python3', 'Scripts/verify-linux-ui.py'], cwd=ROOT, env=env, check=True)
    if args.desktop:
        env['ANYWHERE_ISOLATED_DISPLAY'] = '1'
        subprocess.run(['python3', 'Scripts/verify-linux-desktop.py'], cwd=ROOT, env=env, check=True)
print('PASS deb ownership, layout, dependencies load, packaged Rust/GTK management and Host protocol')
