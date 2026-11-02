#!/usr/bin/env python3
"""Validate the trusted build source and package checksums for CI prereleases."""
import argparse
import hashlib
import json
import os
import pathlib
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.request
import zipfile


def gh_api(endpoint):
    return json.loads(subprocess.check_output(['gh', 'api', endpoint], text=True))


def validate_run(run, repository):
    checks = (
        run.get('repository', {}).get('full_name') == repository,
        run.get('head_repository', {}).get('full_name') == repository,
        run.get('path') == '.github/workflows/build.yml',
        run.get('event') in ('push', 'workflow_dispatch'),
        run.get('head_branch') == 'main',
        run.get('status') == 'completed',
        run.get('conclusion') == 'success',
        re.fullmatch(r'[0-9a-f]{40}', run.get('head_sha', '')) is not None,
    )
    if not all(checks):
        raise ValueError('Only successful Build packages runs from this repository main branch are allowed')
    return run['head_sha']


def source(run_id):
    if not re.fullmatch(r'[1-9][0-9]*', run_id):
        raise ValueError('build_run_id must be a positive integer')
    if os.environ.get('GITHUB_REF') != 'refs/heads/main':
        raise ValueError('Run the signing workflow from main')
    repository = os.environ['GITHUB_REPOSITORY']
    run = gh_api(f'repos/{repository}/actions/runs/{run_id}')
    sha = validate_run(run, repository)
    comparison = gh_api(f'repos/{repository}/compare/{sha}...main')
    if comparison.get('status') not in ('ahead', 'identical'):
        raise ValueError('Build commit is not on current main history')
    with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
        output.write(f'sha={sha}\nrun_id={run_id}\n')
    print(f'Validated main build {run_id} at {sha}')


PATTERNS = {
    'unsigned': r'AnywhereHost-(\d+\.\d+\.\d+)-macos-arm64-unsigned\.zip',
    'notarized': r'AnywhereHost-(\d+\.\d+\.\d+)-macos-arm64-notarized\.zip',
    'amd64': r'anywhere-host_(\d+\.\d+\.\d+)-\d+_amd64\.deb',
    'arm64': r'anywhere-host_(\d+\.\d+\.\d+)-\d+_arm64\.deb',
}


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def package(directory, kind):
    directory = pathlib.Path(directory)
    paths = [path for path in directory.iterdir() if path.name != 'SHA256SUMS']
    if len(paths) != 1 or not re.fullmatch(PATTERNS[kind], paths[0].name):
        raise ValueError(f'Expected exactly one {kind} package in {directory}')
    path = paths[0]
    manifest = directory / 'SHA256SUMS'
    if path.is_symlink() or manifest.is_symlink() or not path.is_file():
        raise ValueError('Artifacts must contain regular package and checksum files')
    lines = manifest.read_text().splitlines()
    if len(lines) != 1:
        raise ValueError('Expected exactly one SHA256SUMS entry')
    match = re.fullmatch(r'([0-9a-f]{64}) [ *](?:\./)?([^/\\]+)', lines[0])
    if not match or match[2] != path.name or match[1] != digest(path):
        raise ValueError(f'Package checksum mismatch: {path.name}')
    return path


def assemble(artifacts, output, sha, run_id):
    if not re.fullmatch(r'[0-9a-f]{40}', sha) or not run_id.isdecimal():
        raise ValueError('Invalid build provenance')
    artifacts, output = pathlib.Path(artifacts), pathlib.Path(output)
    packages = [package(artifacts / kind, kind) for kind in ('notarized', 'amd64', 'arm64')]
    versions = [re.fullmatch(PATTERNS[kind], path.name)[1]
                for kind, path in zip(('notarized', 'amd64', 'arm64'), packages)]
    if len(set(versions)) != 1:
        raise ValueError('Platform package versions differ')
    version = versions[0]
    with zipfile.ZipFile(packages[0]) as archive:
        info = plistlib.loads(archive.read('AnywhereHost.app/Contents/Info.plist'))
    if info.get('CFBundleShortVersionString') != version:
        raise ValueError('Mac archive filename and bundle versions differ')
    deb_versions = set()
    for arch, path in zip(('amd64', 'arm64'), packages[1:]):
        metadata = {}
        for field in ('Package', 'Version', 'Architecture'):
            metadata[field] = subprocess.check_output(['dpkg-deb', '--field', str(path), field], text=True).strip()
        if metadata['Package'] != 'anywhere-host' or metadata['Architecture'] != arch:
            raise ValueError('Wrong Debian package or architecture')
        if not re.fullmatch(re.escape(version) + r'-\d+', metadata['Version']):
            raise ValueError('Wrong Debian version')
        deb_versions.add(metadata['Version'])
    if len(deb_versions) != 1:
        raise ValueError('Debian package revisions differ')
    output.mkdir(parents=True, exist_ok=False)
    for path in packages:
        shutil.copyfile(path, output / path.name)
    (output / 'SHA256SUMS').write_text(''.join(f'{digest(path)}  {path.name}\n' for path in packages))
    repository = os.environ['GITHUB_REPOSITORY']
    tag = f'v{version}-signed.{sha[:12]}'
    notes = (
        f'Commit: `{sha}`\n\n'
        f'[Successful source build](https://github.com/{repository}/actions/runs/{run_id})\n\n'
        '- macOS arm64: Developer ID signed, accepted by Apple notarization, ticket stapled; '
        'verified with codesign, stapler and Gatekeeper.\n'
        '- Linux amd64 / arm64: original Debian 13 packages from the successful source build.\n'
        '- SHA256SUMS covers all three packages.\n\n'
        'This prerelease is for manual installation. It does not replace the latest stable release '
        'or publish a Sparkle appcast / Linux OTA signature.\n'
    )
    (output / 'notes.md').write_text(notes)
    with open(os.environ['GITHUB_OUTPUT'], 'a') as stream:
        stream.write(f'tag={tag}\nversion={version}\n')
    print(f'Prepared {tag}: all three package checksums and versions verified')


def publish(directory, tag, sha):
    directory = pathlib.Path(directory)
    repository = os.environ['GITHUB_REPOSITORY']
    if not re.fullmatch(r'v\d+\.\d+\.\d+-signed\.[0-9a-f]{12}', tag) or not re.fullmatch(r'[0-9a-f]{40}', sha):
        raise ValueError('Invalid release tag or commit')
    assets = sorted([*directory.glob('*.zip'), *directory.glob('*.deb'), directory / 'SHA256SUMS'])
    # gh refuses an existing release: never replace a published package on reruns.
    subprocess.run(['gh', 'release', 'create', tag, '--repo', repository, '--target', sha,
                    '--draft', '--prerelease', '--latest=false', '--title', 'AnywhereHost ' + tag,
                    '--notes-file', str(directory / 'notes.md'), *map(str, assets)], check=True)
    release = gh_api(f'repos/{repository}/releases/tags/{tag}')
    expected = {path.name: 'sha256:' + digest(path) for path in assets}
    actual = {asset['name']: asset.get('digest') for asset in release['assets']}
    if actual != expected or not release['draft'] or not release['prerelease'] or release['target_commitish'] != sha:
        raise ValueError('Draft release provenance/assets differ; left draft unpublished')
    subprocess.run(['gh', 'release', 'edit', tag, '--repo', repository, '--draft=false', '--latest=false'], check=True)
    # Verify the public, anonymous download path after publishing.
    with tempfile.TemporaryDirectory(prefix='anywhere-public-check-') as temporary:
        for asset in release['assets']:
            destination = pathlib.Path(temporary) / asset['name']
            with urllib.request.urlopen(asset['browser_download_url'], timeout=120) as response, destination.open('wb') as stream:
                shutil.copyfileobj(response, stream)
            if 'sha256:' + digest(destination) != expected[asset['name']]:
                raise ValueError('Published download checksum mismatch: ' + asset['name'])
    print('Published and public downloads verified: ' + release['html_url'])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    selection = commands.add_parser('source')
    selection.add_argument('run_id')
    verification = commands.add_parser('package')
    verification.add_argument('directory')
    verification.add_argument('kind', choices=PATTERNS)
    preparation = commands.add_parser('assemble')
    preparation.add_argument('artifacts')
    preparation.add_argument('output')
    preparation.add_argument('sha')
    preparation.add_argument('run_id')
    publication = commands.add_parser('publish')
    publication.add_argument('directory')
    publication.add_argument('tag')
    publication.add_argument('sha')
    args = parser.parse_args()
    if args.command == 'source':
        source(args.run_id)
    elif args.command == 'package':
        print(package(args.directory, args.kind))
    elif args.command == 'assemble':
        assemble(args.artifacts, args.output, args.sha, args.run_id)
    else:
        publish(args.directory, args.tag, args.sha)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
