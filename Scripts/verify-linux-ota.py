#!/usr/bin/env python3
"""Check public update discovery/download using clients extracted from release debs; never install."""
import hashlib
import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch


def verify(package):
    package = Path(package).resolve()
    version = subprocess.check_output(['dpkg-deb', '-f', str(package), 'Version'], text=True).strip()
    architecture = subprocess.check_output(['dpkg-deb', '-f', str(package), 'Architecture'], text=True).strip()
    with tempfile.TemporaryDirectory(prefix='anywhere-ota-client-') as temporary:
        subprocess.run(['dpkg-deb', '-x', str(package), temporary], check=True)
        source = Path(temporary) / 'usr/lib/anywhere-host/anywhere_host/updates.py'
        spec = importlib.util.spec_from_file_location('release_updates_' + architecture, source)
        updates = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(updates)
        updater = updates.Updater(None)
        try:
            # Only the installed version and native architecture are fixtures.
            # Requests, release selection, downloads, metadata and signature checks are real.
            with patch.object(updates, 'native_architecture', return_value=architecture), \
                    patch.object(updater, 'current_version', return_value='0'):
                print(architecture, updater.check(), flush=True)
                if not updater.candidate or updater.candidate['version'] != version:
                    raise ValueError('Packaged updater did not discover the expected latest release')
                print(architecture, updater.fetch(), flush=True)
                if hashlib.sha256(updater.downloaded.read_bytes()).digest() != hashlib.sha256(package.read_bytes()).digest():
                    raise ValueError('Packaged updater downloaded a different release payload')
            with patch.object(updates, 'native_architecture', return_value=architecture), \
                    patch.object(updater, 'current_version', return_value=version):
                updater.check()
                if updater.candidate is not None:
                    raise ValueError('Updater offered the currently installed version again')
        finally:
            updater.close()
    print('PASS packaged ' + architecture + ' updater: public discovery, download, signature and version checks; no installation')


if __name__ == '__main__':
    for package in sys.argv[1:]:
        verify(package)
