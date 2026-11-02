#!/usr/bin/env python3
"""GTK parity regression using a disposable backend and fake Codex, under Xvfb."""
import os
from pathlib import Path
import socket
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, os.environ.get('ANYWHERE_TEST_UI', str(ROOT / 'Linux')))
from anywhere_host.app import GLib, Gio, HostApplication
from anywhere_host.service import HostService, private_json

with tempfile.TemporaryDirectory(prefix='anywhere-gtk-test-') as temporary:
    directory = Path(temporary)
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        port = listener.getsockname()[1]
    private_json(directory / 'config.json', {
        'listen': f'127.0.0.1:{port}', 'codexHome': temporary,
        'codexBinary': str(ROOT / 'Scripts/fake-codex.py'),
    })
    private_json(directory / 'ui.json', {'automaticUpdates': False})
    service = HostService(os.environ.get('ANYWHERE_TEST_BINARY', ROOT / 'target/debug/anywhere-host'), directory)
    app = HostApplication(service)
    app.set_flags(Gio.ApplicationFlags.NON_UNIQUE)
    errors, phase = [], [0]
    deadline = time.monotonic() + 60

    def verify():
        try:
            if time.monotonic() > deadline:
                raise AssertionError('GTK smoke timed out: ' + app.message.get_text())
            if app.busy:
                return GLib.SOURCE_CONTINUE
            if phase[0] == 0 and app.online:
                assert len(list(app.stack.get_pages())) == 8
                assert app.setup_window and not app.preferences.get('onboardingVersion')
                app.setup_next.emit('clicked')
                assert app.setup_step == 1 and not app.setup_next.get_sensitive()
                app.setup_check.emit('clicked')
                phase[0] = 1
            elif phase[0] == 1:
                assert app.checks.ready, app.permission_summary.get_text()
                app.setup_next.emit('clicked')
                assert app.setup_step == 2
                app.setup_next.emit('clicked')
                assert app.setup_window is None and app.preferences.get('onboardingVersion') == 1
                assert service.running and not app.save_button.get_sensitive()
                app.endpoint.set_text(f'https://localhost:{port}')
                app.stack.set_visible_child_name('连接设备')
                app.pair_button.emit('clicked')
                phase[0] = 2
            elif phase[0] == 2:
                assert app.qr.get_paintable() is not None, app.message.get_text()
                assert app.pair_link.get_text().startswith('anywhere-host://connect?')
                app.copy_pair.emit('clicked')
                app.endpoint.set_text(f'https://localhost:{port + 1}')
                assert not app.pair_link.get_visible()
                app.check_path.set_text(temporary)
                app.check_access()
                phase[0] = 3
            elif phase[0] == 3:
                assert '可写：是' in app.permission_result.get_text()
                app.close_window()
                assert service.running and not app.window.get_visible()
                app.activate_action('open', None)
                assert app.window.get_visible() and service.running
                assert app.tray is not None
                # Validate the actual DBusMenu variant, including action IDs.
                assert app.tray.node(0, -1)[2][-1].unpack()[0] == 5
                app.activate_action('permissions', None)
                assert app.stack.get_visible_child_name() == '权限'
                app.activate_action('quit', None)
                return GLib.SOURCE_REMOVE
        except BaseException as error:
            errors.append(error)
            app.request_quit()
            return GLib.SOURCE_REMOVE
        return GLib.SOURCE_CONTINUE

    GLib.timeout_add(100, verify)
    try:
        app.run([])
        app.pool.shutdown(wait=True)
        assert not service.running
        if errors:
            raise errors[0]
        print('PASS GTK setup gating/persistence, 8 management pages, pairing/clipboard, permissions, background close, reopen, tray menu and explicit quit')
    finally:
        service.stop()
        app.updater.close()
        if app.tray:
            app.tray.close()
