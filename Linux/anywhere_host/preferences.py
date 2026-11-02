"""User-owned setup state and cancellable permission probes."""
import json
from pathlib import Path
import threading

from .service import private_json


class Preferences:
    def __init__(self, directory):
        self.path = directory / 'ui.json'
        try:
            self.values = json.loads(self.path.read_text())
        except (OSError, ValueError):
            self.values = {}

    def get(self, name, default=None):
        return self.values.get(name, default)

    def set(self, name, value):
        self.values[name] = value
        private_json(self.path, self.values)


class PermissionChecks:
    def __init__(self, service, preferences):
        self.service, self.preferences = service, preferences
        self.cancelled = threading.Event()
        self.results = []

    def targets(self):
        from gi.repository import GLib
        settings = self.service.settings()
        targets = [('主机数据', str(self.service.directory), True),
                   ('Codex Home', settings.get('codexHome', self.service.default_codex_home()), True)]
        selected = self.preferences.get('permissionDirectory')
        if selected:
            targets.append(('所选目录', selected, True))
        for title, kind in [('文稿', GLib.UserDirectory.DIRECTORY_DOCUMENTS),
                            ('桌面', GLib.UserDirectory.DIRECTORY_DESKTOP),
                            ('下载', GLib.UserDirectory.DIRECTORY_DOWNLOAD)]:
            path = GLib.get_user_special_dir(kind)
            if path:
                targets.append((title, path, False))
        return list({path: (title, path, required) for title, path, required in reversed(targets)}.values())

    def run(self, optional=True):
        self.cancelled.clear()
        self.results = []
        results = []
        for title, path, required in self.targets():
            if self.cancelled.is_set():
                return []
            if not optional and not required:
                continue
            try:
                result = self.service.check_directory(path, self.cancelled)
            except Exception as error:
                result = {'readable': False, 'writable': False, 'issue': str(error)}
            results.append((title, path, required, result))
        self.results = results if not self.cancelled.is_set() else []
        return self.results

    @property
    def ready(self):
        return bool(self.results) and all(not required or (result['readable'] and result['writable'])
                                         for _, _, required, result in self.results)
