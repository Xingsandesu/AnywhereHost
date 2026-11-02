"""Platform paths; never persist build-machine paths in packages."""
import os
from pathlib import Path


def state_directory():
    xdg = Path(os.environ.get('XDG_STATE_HOME', ''))
    base = xdg if xdg.is_absolute() else Path.home() / '.local/state'
    return base / 'AnywhereHost'


def backend_binary():
    bundled = Path(__file__).resolve().parent.parent / 'anywhere-host'
    if bundled.is_file():
        return bundled
    return Path(__file__).resolve().parents[2] / 'target/debug/anywhere-host'


def version():
    installed = Path(__file__).resolve().parent.parent / 'version.txt'
    if installed.exists():
        return installed.read_text().strip()
    import tomllib
    return tomllib.loads((Path(__file__).resolve().parents[2] / 'backend/Cargo.toml').read_text())['package']['version']
