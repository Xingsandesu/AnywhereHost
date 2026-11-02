#!/usr/bin/env python3
"""Validate and publish trusted CI packages, including signed stable OTA releases."""
import argparse
import hashlib
import io
import json
import os
import pathlib
import plistlib
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
import zipfile
import ota


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


def source(run_id, stable=False):
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
    if stable and (comparison.get('status') != 'identical' or sha != os.environ['GITHUB_SHA']):
        raise ValueError('Stable releases require a successful build of the current main commit')
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


def assemble(artifacts, output, sha, run_id, stable=False):
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
        if stable:
            data = subprocess.check_output(['dpkg-deb', '--fsys-tarfile', str(path)])
            with tarfile.open(fileobj=io.BytesIO(data)) as contents:
                key = contents.getmember('./usr/lib/anywhere-host/update-public-key.pem')
                if not key.isfile() or contents.extractfile(key).read() != (ota.ROOT / 'Linux/packaging/update-public-key.pem').read_bytes():
                    raise ValueError('Debian package uses a different OTA public key')
    if len(deb_versions) != 1:
        raise ValueError('Debian package revisions differ')
    tag = f'v{version}' if stable else f'v{version}-signed.{sha[:12]}'
    if stable:
        metadata = artifacts / 'ota'
        expected = {'appcast.xml', *(path.name + '.sig' for path in packages[1:])}
        if {path.name for path in metadata.iterdir()} != expected or any(path.is_symlink() or not path.is_file() for path in metadata.iterdir()):
            raise ValueError('Incomplete or unexpected OTA metadata')
        ota.verify(metadata, packages[0], packages[1:], tag)
        packages += [metadata / name for name in sorted(expected)]
    output.mkdir(parents=True, exist_ok=False)
    for path in packages:
        shutil.copyfile(path, output / path.name)
    (output / 'SHA256SUMS').write_text(''.join(f'{digest(path)}  {path.name}\n' for path in packages))
    repository = os.environ['GITHUB_REPOSITORY']
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
    if stable:
        notes = (ota.ROOT / f'Docs/Releases/{version}.md').read_text() + (
            f'\n\nSource commit: `{sha}`\n\n'
            f'[Successful source build](https://github.com/{repository}/actions/runs/{run_id})\n\n'
            'Developer ID signing, Apple notarization, Gatekeeper, Sparkle Ed25519 and both Debian '
            'Ed25519 signatures are verified before publishing. SHA256SUMS covers every release asset.\n')
    (output / 'notes.md').write_text(notes)
    with open(os.environ['GITHUB_OUTPUT'], 'a') as stream:
        stream.write(f'tag={tag}\nversion={version}\n')
    print(f'Prepared {tag}: all three package checksums and versions verified')


def publish(directory, tag, sha, stable=False):
    directory = pathlib.Path(directory)
    repository = os.environ['GITHUB_REPOSITORY']
    pattern = r'v\d+\.\d+\.\d+' if stable else r'v\d+\.\d+\.\d+-signed\.[0-9a-f]{12}'
    if not re.fullmatch(pattern, tag) or not re.fullmatch(r'[0-9a-f]{40}', sha):
        raise ValueError('Invalid release tag or commit')
    assets = sorted([*directory.glob('*.zip'), *directory.glob('*.deb'), directory / 'SHA256SUMS'])
    if stable:
        archives, debs = list(directory.glob('*.zip')), sorted(directory.glob('*.deb'))
        if len(archives) != 1 or len(debs) != 2:
            raise ValueError('Stable release requires all three platforms')
        ota.verify(directory, archives[0], debs, tag)
        if gh_api(f'repos/{repository}/git/ref/heads/main')['object']['sha'] != sha:
            raise ValueError('Main changed after the build; rebuild before a stable release')
        latest = gh_api(f'repos/{repository}/releases/latest')
        version_tuple = lambda value: tuple(map(int, value.removeprefix('v').split('.')))
        if version_tuple(tag) <= version_tuple(latest['tag_name']):
            raise ValueError('Stable version must increase beyond the current latest release')
        feed = next(asset for asset in latest['assets'] if asset['name'] == 'appcast.xml')
        with urllib.request.urlopen(feed['browser_download_url'], timeout=30) as response:
            previous = ota.ET.fromstring(response.read()).findtext('./channel/item/{' + ota.NAMESPACE + '}version')
        if int(ota.app_info(archives[0])['CFBundleVersion']) <= int(previous):
            raise ValueError('Mac build number must increase beyond the published OTA build')
        assets += [directory / 'appcast.xml', *(directory / (path.name + '.sig') for path in debs)]
    # gh refuses an existing release: never replace a published package on reruns.
    subprocess.run(['gh', 'release', 'create', tag, '--repo', repository, '--target', sha,
                    '--draft', *([] if stable else ['--prerelease']), '--latest=false', '--title', 'AnywhereHost ' + tag,
                    '--notes-file', str(directory / 'notes.md'), *map(str, assets)], check=True)
    # REST's tag endpoint only finds published tags. gh also resolves a draft's
    # pending tag through GraphQL; use that ID for the authoritative REST data.
    release_id = json.loads(subprocess.check_output(
        ['gh', 'release', 'view', tag, '--repo', repository, '--json', 'databaseId'], text=True))['databaseId']
    release = gh_api(f'repos/{repository}/releases/{release_id}')
    expected = {path.name: 'sha256:' + digest(path) for path in assets}
    actual = {asset['name']: asset.get('digest') for asset in release['assets']}
    if actual != expected or not release['draft'] or release['prerelease'] != (not stable) or release['target_commitish'] != sha:
        raise ValueError('Draft release provenance/assets differ; left draft unpublished')
    subprocess.run(['gh', 'release', 'edit', tag, '--repo', repository, '--draft=false',
                    '--latest=true' if stable else '--latest=false'], check=True)
    # Publishing replaces the draft's untagged-* URLs with the real tag URLs.
    release = gh_api(f'repos/{repository}/releases/{release_id}')
    if release['draft'] or release['tag_name'] != tag:
        raise ValueError('Release did not become public under the expected tag')
    # Verify the public, anonymous download path after publishing.
    with tempfile.TemporaryDirectory(prefix='anywhere-public-check-') as temporary:
        for asset in release['assets']:
            destination = pathlib.Path(temporary) / asset['name']
            with urllib.request.urlopen(asset['browser_download_url'], timeout=120) as response, destination.open('wb') as stream:
                shutil.copyfileobj(response, stream)
            if 'sha256:' + digest(destination) != expected[asset['name']]:
                raise ValueError('Published download checksum mismatch: ' + asset['name'])
        if stable:
            public = pathlib.Path(temporary)
            ota.verify(public, public / archives[0].name, [public / path.name for path in debs], tag)
            if gh_api(f'repos/{repository}/releases/latest')['id'] != release_id:
                raise ValueError('Published stable release is not GitHub Latest')
            subprocess.run([sys.executable, str(ota.ROOT / 'Scripts/verify-linux-ota.py'),
                            *(str(public / path.name) for path in debs)], check=True)
    print('Published and public downloads verified: ' + release['html_url'])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    selection = commands.add_parser('source')
    selection.add_argument('run_id')
    selection.add_argument('--stable', action='store_true')
    verification = commands.add_parser('package')
    verification.add_argument('directory')
    verification.add_argument('kind', choices=PATTERNS)
    preparation = commands.add_parser('assemble')
    preparation.add_argument('artifacts')
    preparation.add_argument('output')
    preparation.add_argument('sha')
    preparation.add_argument('run_id')
    preparation.add_argument('--stable', action='store_true')
    publication = commands.add_parser('publish')
    publication.add_argument('directory')
    publication.add_argument('tag')
    publication.add_argument('sha')
    publication.add_argument('--stable', action='store_true')
    args = parser.parse_args()
    if args.command == 'source':
        source(args.run_id, args.stable)
    elif args.command == 'package':
        print(package(args.directory, args.kind))
    elif args.command == 'assemble':
        assemble(args.artifacts, args.output, args.sha, args.run_id, args.stable)
    else:
        publish(args.directory, args.tag, args.sha, args.stable)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
