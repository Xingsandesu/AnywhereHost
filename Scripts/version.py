#!/usr/bin/env python3
"""Check or update the Host's Cargo and Xcode release versions together."""
import argparse
import json
import pathlib
import os
import re

root = pathlib.Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--set', dest='version')
parser.add_argument('--build', type=int)
parser.add_argument('--client-root', type=pathlib.Path, default=os.environ.get('ANYWHERE_CLIENT_SOURCE'),
                    help='Optional AnyWhereCoding checkout to validate or update')
args = parser.parse_args()
client_path = args.client_root.resolve() / 'AnyWhereCoding.xcodeproj/project.pbxproj' if args.client_root else None
if client_path and not client_path.is_file():
    parser.error('--client-root must contain AnyWhereCoding.xcodeproj')
cargo_path = root / 'backend/Cargo.toml'
project_path = root / 'AnywhereHost.xcodeproj/project.pbxproj'
cargo, project = cargo_path.read_text(), project_path.read_text()
if args.version:
    if not re.fullmatch(r'\d+\.\d+\.\d+', args.version) or not args.build or args.build < 1:
        parser.error('--set requires x.y.z and a positive --build')
    cargo = re.sub(r'^version = "[^"]+"', f'version = "{args.version}"', cargo, count=1, flags=re.M)
    project = re.sub(r'MARKETING_VERSION = [^;]+;', f'MARKETING_VERSION = {args.version};', project)
    project = re.sub(r'CURRENT_PROJECT_VERSION = [^;]+;', f'CURRENT_PROJECT_VERSION = {args.build};', project)
    native_path = root / 'transport/Cargo.toml'
    native_path.write_text(re.sub(r'^version = "[^"]+"', f'version = "{args.version}"', native_path.read_text(), count=1, flags=re.M))
    if client_path:
        client_text = re.sub(r'MARKETING_VERSION = [^;]+;', f'MARKETING_VERSION = {args.version};', client_path.read_text())
        client_text = re.sub(r'CURRENT_PROJECT_VERSION = [^;]+;', f'CURRENT_PROJECT_VERSION = {args.build};', client_text)
        client_path.write_text(client_text)
    lock_path = root / 'Cargo.lock'
    lock_path.write_text(re.sub(r'(name = "anywhere-(?:host|transport)"\nversion = ")[^"]+',
                                lambda match: match[1] + args.version, lock_path.read_text()))
    for name in ['package.json', 'package-lock.json']:
        path = root / 'notification-worker' / name
        package = json.loads(path.read_text())
        package['version'] = args.version
        if 'packages' in package:
            package['packages']['']['version'] = args.version
        path.write_text(json.dumps(package, indent=2) + '\n')
    cargo_path.write_text(cargo)
    project_path.write_text(project)
version = re.search(r'^version = "([^"]+)"', cargo, re.M).group(1)
assert set(re.findall(r'MARKETING_VERSION = ([^;]+);', project)) == {version}, 'Cargo/Xcode versions differ'
builds = set(re.findall(r'CURRENT_PROJECT_VERSION = ([^;]+);', project))
assert len(builds) == 1 and next(iter(builds)).isdigit(), 'Xcode build numbers differ'
print(f'{version} ({next(iter(builds))})')

if client_path:
    text = client_path.read_text()
    assert set(re.findall(r'MARKETING_VERSION = ([^;]+);', text)) == {version}
    client_builds = set(re.findall(r'CURRENT_PROJECT_VERSION = ([^;]+);', text))
    assert len(client_builds) == 1 and next(iter(client_builds)).isdigit(), 'Client app/extension build numbers differ'
    # Xcode Cloud allocates the client build number independently of Host releases.

assert re.search(r'^version = "([^"]+)"', (root / 'transport/Cargo.toml').read_text(), re.M).group(1) == version

for name in ['package.json', 'package-lock.json']:
    package = json.loads((root / 'notification-worker' / name).read_text())
    assert package['version'] == version, 'Worker version differs'
    if 'packages' in package:
        assert package['packages']['']['version'] == version
assert set(re.findall(r'name = "anywhere-(?:host|transport)"\nversion = "([^"]+)"', (root / 'Cargo.lock').read_text())) == {version}, 'Cargo lock versions differ'
