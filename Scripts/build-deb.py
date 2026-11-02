#!/usr/bin/env python3
"""Build a native Debian 13 amd64/arm64 package without root or user-specific paths."""
import argparse
import os
from pathlib import Path
import platform
import base64
import plistlib
import shutil
import shlex
import subprocess
import tempfile
import tomllib
import importlib.util

ROOT = Path(__file__).resolve().parents[1]
TARGETS = {'amd64': 'x86_64-unknown-linux-gnu', 'arm64': 'aarch64-unknown-linux-gnu'}


def run(*args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--desktop-container', action='store_true', help='Build desktop helpers in a Debian container')
    parser.add_argument('--output-dir', type=Path, default=ROOT / 'dist')
    args = parser.parse_args()
    release = platform.freedesktop_os_release()
    arch = subprocess.check_output(['dpkg', '--print-architecture'], text=True).strip()
    if release.get('ID') != 'debian' or release.get('VERSION_ID') != '13' or arch not in TARGETS:
        parser.error('Packaging requires native Debian 13 amd64 or arm64')
    key = plistlib.loads((ROOT / 'Resources/Info.plist').read_bytes())['SUPublicEDKey']
    pem = (ROOT / 'Linux/packaging/update-public-key.pem').read_text().splitlines()
    if base64.b64decode(''.join(pem[1:-1])) != bytes.fromhex('302a300506032b6570032100') + base64.b64decode(key):
        raise RuntimeError('Linux and macOS update identities differ')
    version = tomllib.loads((ROOT / 'backend/Cargo.toml').read_text())['package']['version']
    revision = (ROOT / 'Linux/packaging/revision').read_text().strip()
    if not revision.isascii() or not revision.isdigit() or int(revision) < 1:
        raise RuntimeError('Debian package revision must be a positive integer')
    package_version = f'{version}-{revision}'
    target = TARGETS[arch]
    # Explicit target avoids an ambient cargo configuration silently cross-compiling.
    flags = (os.environ['CARGO_ENCODED_RUSTFLAGS'].split('\x1f') if os.environ.get('CARGO_ENCODED_RUSTFLAGS')
             else shlex.split(os.environ.get('RUSTFLAGS', '')))
    flags += [f'--remap-path-prefix={Path.home()}=/build/home', f'--remap-path-prefix={ROOT}=/usr/src/anywhere-host']
    build_env = {**os.environ, 'CARGO_ENCODED_RUSTFLAGS': '\x1f'.join(flags)}
    run('cargo', 'build', '--release', '--locked', '-p', 'anywhere-host', '--bins', '--target', target,
        '--target-dir', str(ROOT / 'target'), cwd=ROOT, env=build_env)
    spec = importlib.util.spec_from_file_location('desktop_builder', ROOT / 'Scripts/build-linux-desktop.py')
    desktop_builder = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(desktop_builder)
    desktop_source = desktop_builder.build(args.desktop_container)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    output = args.output_dir.resolve() / f'anywhere-host_{package_version}_{arch}.deb'
    with tempfile.TemporaryDirectory(prefix='anywhere-deb-') as temporary:
        root = Path(temporary)
        stage = root / 'debian/anywhere-host'
        lib = stage / 'usr/lib/anywhere-host'
        lib.mkdir(parents=True)
        (lib / 'version.txt').write_text(package_version + '\n')
        for binary in ('anywhere-host', 'anywhere-permission-probe'):
            shutil.copy2(ROOT / 'target' / target / 'release' / binary, lib / binary)
        for name in ('w0vncserver', 'x0vncserver', 'vncpasswd'):
            shutil.copy2(desktop_source / 'build/unix' / name / name, lib / name)
        run('python3', str(ROOT / 'Scripts/fetch-frpc.py'), '--output-dir', str(lib))
        shutil.copytree(ROOT / 'Linux/anywhere_host', lib / 'anywhere_host',
                        ignore=shutil.ignore_patterns('__pycache__', '*.pyc'))
        paths = {
            'Linux/packaging/anywhere-host-gtk': 'usr/bin/anywhere-host-gtk',
            'Linux/packaging/com.jintongshu.AnywhereHost.desktop':
                'usr/share/applications/com.jintongshu.AnywhereHost.desktop',
            'Resources/AppIcon.iconset/icon_512x512.png':
                'usr/share/icons/hicolor/512x512/apps/com.jintongshu.AnywhereHost.png',
            'Docs/LINUX.md': 'usr/share/doc/anywhere-host/README.md',
            'Linux/packaging/install-update': 'usr/lib/anywhere-host/install-update',
            'Linux/packaging/update-public-key.pem': 'usr/lib/anywhere-host/update-public-key.pem',
            'Linux/packaging/com.jintongshu.AnywhereHost.policy': 'usr/share/polkit-1/actions/com.jintongshu.AnywhereHost.policy',
            'Linux/packaging/copyright': 'usr/share/doc/anywhere-host/copyright',
        }
        for source, destination in paths.items():
            destination = stage / destination
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / source, destination)
        shutil.copy2(ROOT / '.build' / f'tigervnc-{desktop_builder.VERSION}.tar.gz', stage / 'usr/share/doc/anywhere-host/tigervnc-source.tar.gz')
        shutil.copy2(ROOT / 'Scripts/build-linux-desktop.py', stage / 'usr/share/doc/anywhere-host/build-linux-desktop.py')
        shutil.copy2(desktop_source / 'LICENCE.TXT', stage / 'usr/share/doc/anywhere-host/TigerVNC-LICENSE')
        shutil.move(lib / 'frp-LICENSE', stage / 'usr/share/doc/anywhere-host/frp-LICENSE')
        # Resolve actual ELF dependencies from the native Debian package database.
        (root / 'debian').mkdir(exist_ok=True)
        (root / 'debian/control').write_text(f'Source: anywhere-host\n\nPackage: anywhere-host\nArchitecture: {arch}\nDescription: Host service\n')
        result = subprocess.check_output(['dpkg-shlibdeps', '-O', '-e' + str(lib / 'anywhere-host'),
            '-e' + str(lib / 'anywhere-permission-probe'), *['-e' + str(lib / name) for name in ('w0vncserver', 'x0vncserver', 'vncpasswd')]], cwd=root, text=True)
        dependencies = next(line.removeprefix('shlibs:Depends=') for line in result.splitlines()
                            if line.startswith('shlibs:Depends='))
        control = stage / 'DEBIAN'
        control.mkdir()
        size = sum(p.stat().st_size for p in stage.rglob('*') if p.is_file()) // 1024
        (control / 'control').write_text(f'''Package: anywhere-host
Version: {package_version}
Section: net
Priority: optional
Architecture: {arch}
Maintainer: AnywhereHost contributors <noreply@github.com>
Depends: {dependencies}, python3 (>= 3.11), python3-gi, python3-qrcode, python3-cryptography (>= 43), gir1.2-gtk-4.0 (>= 4.18), openssl, git, pkexec, xdg-desktop-portal, xdg-desktop-portal-gnome, gnome-control-center
Installed-Size: {size}
Recommends: gnome-shell-extension-appindicator
Homepage: https://github.com/Xingsandesu/AnywhereHost
Description: GTK management console and Rust backend for remote Codex workflows
 Native Debian 13 host management, secure device pairing and frp tunnels.
 Requires a separately installed Codex runtime. Runs as the desktop user.
''')
        # Normalize modes, independent of the builder's umask; dpkg normalizes ownership.
        stage.chmod(0o755)
        for path in stage.rglob('*'):
            path.chmod(0o755 if path.is_dir() else 0o644)
        for binary in ('anywhere-host', 'anywhere-permission-probe', 'frpc', 'w0vncserver', 'x0vncserver', 'vncpasswd', 'install-update'):
            (lib / binary).chmod(0o755)
        (stage / 'usr/bin/anywhere-host-gtk').chmod(0o755)
        epoch = subprocess.check_output(['git', 'log', '-1', '--format=%ct'], cwd=ROOT, text=True).strip()
        run('dpkg-deb', '--root-owner-group', '--build', str(stage), str(output),
            env={**os.environ, 'SOURCE_DATE_EPOCH': os.environ.get('SOURCE_DATE_EPOCH', epoch)})
    print(output)


if __name__ == '__main__':
    main()
