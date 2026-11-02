#!/usr/bin/env python3
"""Sign a release deb with the existing Host Ed25519 identity (PEM private key)."""
import argparse
import base64
from pathlib import Path
import subprocess
import sys
import tempfile
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'Linux'))
from anywhere_host.updates import verify
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('package', type=Path)
parser.add_argument('--key', type=Path, required=True)
args = parser.parse_args()
with tempfile.NamedTemporaryFile() as signature:
    subprocess.run(['openssl', 'pkeyutl', '-sign', '-inkey', str(args.key), '-rawin',
                    '-in', str(args.package), '-out', signature.name], check=True)
    destination = args.package.with_suffix('.deb.sig')
    destination.write_text(base64.b64encode(Path(signature.name).read_bytes()).decode() + '\n')
    try:
        verify(args.package, destination)
    except Exception:
        destination.unlink()
        raise
print(destination)
