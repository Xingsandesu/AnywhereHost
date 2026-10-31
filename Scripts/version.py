#!/usr/bin/env python3
"""Check or update the Host's Cargo and Xcode release versions together."""
import argparse
import pathlib
import re

root = pathlib.Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--set', dest='version')
parser.add_argument('--build', type=int)
args = parser.parse_args()
cargo_path = root / 'backend/Cargo.toml'
project_path = root / 'AnywhereHost.xcodeproj/project.pbxproj'
cargo, project = cargo_path.read_text(), project_path.read_text()
if args.version:
    if not re.fullmatch(r'\d+\.\d+\.\d+', args.version) or not args.build or args.build < 1:
        parser.error('--set requires x.y.z and a positive --build')
    cargo = re.sub(r'^version = "[^"]+"', f'version = "{args.version}"', cargo, count=1, flags=re.M)
    project = re.sub(r'MARKETING_VERSION = [^;]+;', f'MARKETING_VERSION = {args.version};', project)
    project = re.sub(r'CURRENT_PROJECT_VERSION = [^;]+;', f'CURRENT_PROJECT_VERSION = {args.build};', project)
    cargo_path.write_text(cargo)
    project_path.write_text(project)
version = re.search(r'^version = "([^"]+)"', cargo, re.M).group(1)
assert set(re.findall(r'MARKETING_VERSION = ([^;]+);', project)) == {version}, 'Cargo/Xcode versions differ'
builds = set(re.findall(r'CURRENT_PROJECT_VERSION = ([^;]+);', project))
assert len(builds) == 1 and next(iter(builds)).isdigit(), 'Xcode build numbers differ'
print(f'{version} ({next(iter(builds))})')
