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
                app.stack.set_visible_child_name('远程桌面')
                app.stop_button.emit('clicked')
                phase[0] = 4
            elif phase[0] == 4:
                assert not service.running and not app.online
                assert not app.desktop_enable.get_sensitive()
                assert app.desktop_password_save.get_sensitive(), 'Saving must work with Host stopped'
                app.desktop_password.set_text('short')
                app.desktop_password_save.emit('clicked')
                phase[0] = 5
            elif phase[0] == 5:
                assert '密码未保存' in app.desktop_password_status.get_text()
                assert app.desktop_password.get_text() == 'short', 'Keep failed input for correction'
                assert not service.has_desktop_password()
                app.desktop_password.set_text('test1234')
                app.desktop_password_save.emit('clicked')
                phase[0] = 6
            elif phase[0] == 6:
                assert service.has_desktop_password(), app.message.get_text()
                assert not (directory / 'desktop.json').exists(), 'Saving must not enable sharing'
                assert app.desktop_password.get_text() == ''
                assert '密码已保存' in app.desktop_password_status.get_text()
                app.start_button.emit('clicked')
                phase[0] = 7
            elif phase[0] == 7 and app.online:
                assert app.desktop_enable.get_sensitive(), app.desktop_status.get_text()
                app.desktop_enable.emit('clicked')
                phase[0] = 8
            elif phase[0] == 8:
                assert app.desktop_enabled, app.message.get_text()
                assert not app.desktop_enable.get_sensitive()
                assert app.desktop_password_save.get_sensitive(), 'Saving must work with sharing enabled'
                app.desktop_password.set_text('new12345')
                app.desktop_password_save.emit('clicked')
                phase[0] = 9
            elif phase[0] == 9:
                assert app.desktop_password.get_text() == ''
                assert '密码已保存' in app.desktop_password_status.get_text()
                assert app.desktop_enabled
                app.desktop_disable.emit('clicked')
                phase[0] = 10
            elif phase[0] == 10:
                assert not app.desktop_enabled, app.message.get_text()
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
        print('PASS GTK setup, 8 pages, pairing, permissions, tray, offline password save, validation errors, password change while sharing and explicit quit')
    finally:
        service.stop()
        app.updater.close()
        if app.tray:
            app.tray.close()
