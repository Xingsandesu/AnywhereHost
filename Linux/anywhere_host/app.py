"""GTK 4 dashboard. All network and process waits run off the GTK thread."""
import argparse
import os
from concurrent.futures import ThreadPoolExecutor
import signal
import time
from pathlib import Path

import gi
gi.require_version('Gtk', '4.0')
gi.require_version('Gdk', '4.0')
from gi.repository import Gdk, Gio, GLib, Gtk

from .client import HostClient
from .paths import backend_binary, state_directory, version
from .service import HostService
from .preferences import Preferences, PermissionChecks
from .updates import Updater
from .panels import ManagementPanels
from .tray import Tray


def label(text='', style=None):
    widget = Gtk.Label(label=text, xalign=0, wrap=True, selectable=True)
    if style:
        widget.add_css_class(style)
    return widget


class HostApplication(Gtk.Application, ManagementPanels):
    def __init__(self, service):
        super().__init__(application_id='com.jintongshu.AnywhereHost', flags=Gio.ApplicationFlags.DEFAULT_FLAGS)
        self.service = service
        self.preferences = Preferences(service.directory)
        self.checks = PermissionChecks(service, self.preferences)
        self.updater = Updater(service)
        self.restart_requested = False
        self.tray = None
        self.codex_connected = False
        self.desktop_supported = False
        self.desktop_enabled = False
        self.revoke_buttons = []
        self.permission_recheck = self.preferences.get('onboardingVersion', 0) >= 1
        self.pair_refresh_needed = False
        self.pool = ThreadPoolExecutor(max_workers=1, thread_name_prefix='host-management')
        self.busy = False
        self.closing = False
        self.online = False
        self.window = None
        self.actions = []
        self.pair_expiry = 0
        self.device_ids = None
        self.tunnel_loaded = False
        for name, callback in {
            'open': lambda: self.activate_window(),
            'permissions': lambda: self.open_page('权限'),
            'setup': lambda: (self.activate_window(), self.open_setup()),
            'updates': lambda: (self.open_page('更新'), self.check_updates()),
            'quit': self.request_quit,
        }.items():
            action = Gio.SimpleAction.new(name, None)
            action.connect('activate', lambda _a, _p, callback=callback: callback())
            self.add_action(action)
        self.set_accels_for_action('app.quit', ['<Primary>q'])
        self.connect('activate', self.activate_window)

    def button(self, title, callback, primary=False):
        button = Gtk.Button(label=title)
        if primary:
            button.add_css_class('suggested-action')
        button.connect('clicked', lambda _: callback())
        self.actions.append(button)
        return button

    def page(self, title, description):
        page = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=16)
        for edge in ('top', 'bottom', 'start', 'end'):
            getattr(page, f'set_margin_{edge}')(24)
        page.append(label(title, 'title-1'))
        page.append(label(description, 'dim-label'))
        scroll = Gtk.ScrolledWindow(vexpand=True, hexpand=True)
        scroll.set_child(page)
        self.stack.add_titled(scroll, title, title)
        return page

    def field(self, page, title, value='', secret=False):
        row = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        row.append(label(title))
        entry = Gtk.PasswordEntry(show_peek_icon=True) if secret else Gtk.Entry()
        entry.set_text(value)
        entry.set_hexpand(True)
        row.append(entry)
        page.append(row)
        return entry

    def activate_window(self, *_):
        if self.window:
            self.window.present()
            return
        self.window = Gtk.ApplicationWindow(application=self, title='AnywhereHost')
        self.window.set_default_size(960, 740)
        self.window.connect('close-request', self.close_window)
        header = Gtk.HeaderBar()
        header.set_title_widget(Gtk.Label(label=f'AnywhereHost {version()} · Linux'))
        menu = Gio.Menu()
        for title, action in [('打开主机', 'open'), ('权限…', 'permissions'), ('设置向导…', 'setup'),
                              ('检查更新…', 'updates'), ('退出 AnywhereHost', 'quit')]:
            menu.append(title, 'app.' + action)
        header.pack_end(Gtk.MenuButton(icon_name='open-menu-symbolic', menu_model=menu))
        self.window.set_titlebar(header)
        root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        self.message = label('就绪。启动服务以连接已安装的 Codex。')
        self.message.set_margin_top(10)
        self.message.set_margin_bottom(10)
        self.message.set_margin_start(20)
        self.message.set_margin_end(20)
        root.append(self.message)
        content = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, vexpand=True)
        self.stack = Gtk.Stack(transition_type=Gtk.StackTransitionType.CROSSFADE)
        sidebar = Gtk.StackSidebar(stack=self.stack)
        sidebar.set_size_request(170, -1)
        content.append(sidebar)
        content.append(Gtk.Separator(orientation=Gtk.Orientation.VERTICAL))
        content.append(self.stack)
        root.append(content)
        self.window.set_child(root)

        page = self.page('服务概览', '管理此设备的 Host 服务。项目、会话、模型和审批由远程客户端操作。')
        self.status = label('服务未连接', 'title-2')
        page.append(self.status)
        self.runtime = label('')
        page.append(self.runtime)
        row = Gtk.Box(spacing=10)
        self.start_button = self.button('启动服务', self.start, True)
        self.stop_button = self.button('停止服务', self.stop)
        self.connect_button = self.button('重新连接 Codex', lambda: self.api('/v1/runtime/connect', {}))
        for button in (self.start_button, self.stop_button, self.connect_button):
            row.append(button)
        page.append(row)
        page.append(label(f'状态目录：{self.service.directory}'))
        page.append(label('关闭窗口后服务继续运行。通过应用菜单或托盘“退出”停止本界面启动的服务。', 'dim-label'))
        page.append(self.button('打开主机数据', lambda: self.open_file(self.service.directory)))
        page.append(self.button('设置向导', self.open_setup))
        self.tray_status = label('')
        page.append(self.tray_status)
        page.append(self.button('启用 GNOME 状态图标', self.enable_tray))

        page = self.page('连接设备', '使用客户端扫描一次性配对二维码；五分钟有效。生成新二维码会替换旧票据。')
        self.endpoint = self.field(page, '客户端可访问的 HTTPS 地址', 'https://')
        self.endpoint.connect('changed', lambda _: self.clear_pair())
        self.pair_button = self.button('生成配对二维码', self.pair, True)
        page.append(self.pair_button)
        self.qr = Gtk.Picture(can_shrink=True)
        self.qr.set_size_request(300, 300)
        self.qr.set_halign(Gtk.Align.CENTER)
        self.qr.set_visible(False)
        page.append(self.qr)
        self.pair_status = label('')
        page.append(self.pair_status)
        self.pair_link = Gtk.Entry(editable=False, visible=False)
        page.append(self.pair_link)
        self.copy_pair = self.button('复制连接链接', self.copy_pair_link)
        page.append(self.copy_pair)
        page.append(label('已配对设备', 'title-3'))
        self.devices = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        page.append(self.devices)

        page = self.page('隧道', '连接你自己的 frps。进程运行不等于公网可达，请从远程客户端验证连接。')
        self.tunnel_status = label('隧道状态未知', 'title-3')
        page.append(self.tunnel_status)
        self.server = self.field(page, 'frps 地址')
        self.server_port = self.field(page, '控制端口', '7000')
        self.remote_port = self.field(page, 'Host 映射端口', '9443')
        self.token = self.field(page, 'Token（已保存时可留空）', secret=True)
        self.ca = self.field(page, 'frps CA 文件绝对路径（可选）')
        page.append(label('未指定 CA 时，frp 仅加密传输，不验证 frps 身份；Host HTTPS 仍使用证书固定验证。', 'dim-label'))
        row = Gtk.Box(spacing=10)
        self.tunnel_start = self.button('启动隧道', self.start_tunnel, True)
        self.tunnel_stop = self.button('停止隧道', lambda: self.api('/v1/tunnel/stop', {}))
        row.append(self.tunnel_start)
        row.append(self.tunnel_stop)
        page.append(row)
        self.tunnel_log = label('')
        page.append(self.tunnel_log)

        page = self.page('运行时与权限', '服务停止后可修改设置。保留实际 Codex Home、登录、项目路径与会话 ID。')
        try:
            settings = self.service.settings()
        except (OSError, ValueError) as error:
            settings = {}
            self.message.set_text(f'无法读取配置：{error}')
        self.codex_home = self.field(page, 'Codex Home', str(settings.get('codexHome', self.service.default_codex_home())))
        self.codex_binary = self.field(page, 'Codex 可执行文件（留空自动发现）', settings.get('codexBinary') or '')
        self.control_socket = self.field(page, '已有 Codex 控制 socket（留空自动发现）', settings.get('controlSocket') or '')
        self.listen_port = self.field(page, '本地 HTTPS 端口', settings.get('listen', '127.0.0.1:9443').rsplit(':', 1)[1])
        self.save_button = self.button('保存设置', self.save)
        page.append(self.save_button)
        self.check_path = self.field(page, '检查目录读写权限（只创建并移除临时文件）', str(Path.home()))
        page.append(self.button('检查目录权限', self.check_access))
        self.permission_result = label('')
        page.append(self.permission_result)
        page.append(label('目录检查只反映 Host 当前用户的权限，不代表共享 Codex 进程或其他工具已获得访问权限。', 'dim-label'))

        page = self.page('日志', '仅显示 Host 日志末尾 16 KiB；日志可能包含本机路径，分享前请检查。')
        page.append(self.button('刷新日志', lambda: self.work(self.service.log_tail, self.show_log)))
        self.log_view = Gtk.TextView(editable=False, cursor_visible=False, monospace=True, wrap_mode=Gtk.WrapMode.WORD_CHAR)
        log_scroll = Gtk.ScrolledWindow(vexpand=True, min_content_height=450)
        log_scroll.set_child(self.log_view)
        page.append(log_scroll)
        self.build_panels()
        self.update_buttons()
        self.hold()
        try:
            self.tray = Tray(self)
        except GLib.Error:
            pass
        self.window.present()
        GLib.timeout_add_seconds(3, self.tick)
        if (self.service.directory / 'config.json').exists() or self.permission_recheck:
            self.start()
        if not self.preferences.get('onboardingVersion', 0):
            self.open_setup()
        GLib.timeout_add_seconds(15, self.automatic_update_check)

    def work(self, operation, done=None, quiet=False):
        if self.busy or self.closing:
            return
        self.busy = True
        self.update_buttons()
        future = self.pool.submit(operation)
        def completed(future):
            def deliver():
                self.busy = False
                if self.closing:
                    return GLib.SOURCE_REMOVE
                try:
                    result = future.result()
                    if done:
                        done(result)
                    elif not quiet:
                        self.message.set_text('操作完成')
                except Exception as error:
                    if quiet:
                        self.online = False
                        self.codex_connected = False
                        self.status.set_text('服务未连接' if not self.service.running else '服务正在启动或暂不可用')
                        self.runtime.set_text(str(error))
                        self.clear_pair()
                    else:
                        self.message.set_text(f'操作失败：{error}')
                self.update_buttons()
                return GLib.SOURCE_REMOVE
            GLib.idle_add(deliver)
        future.add_done_callback(completed)

    def update_buttons(self):
        for button in self.actions:
            button.set_sensitive(not self.busy and not self.closing)
        ready = not self.busy and not self.closing
        self.start_button.set_sensitive(ready and not self.online and not self.service.running)
        self.stop_button.set_sensitive(ready and self.service.running)
        self.save_button.set_sensitive(ready and not self.online and not self.service.running)
        for button in (self.connect_button, self.pair_button, self.tunnel_start, self.tunnel_stop, *self.revoke_buttons):
            button.set_sensitive(ready and self.online)
        self.desktop_enable.set_sensitive(ready and self.online and self.desktop_supported and not self.desktop_enabled)
        self.desktop_disable.set_sensitive(ready and self.online and self.desktop_enabled)
        self.download_button.set_sensitive(ready and self.updater.candidate is not None)
        self.install_button.set_sensitive(ready and self.updater.downloaded is not None)
        self.copy_pair.set_sensitive(bool(self.pair_link.get_text()))
        self.update_setup()

    def api(self, path, body):
        self.work(lambda: HostClient(self.service.directory).request(path, body))

    def refresh(self):
        if not self.service.running and not (self.service.directory / 'config.json').exists():
            return
        def read():
            client = HostClient(self.service.directory)
            return (client.request('/v1/status'), client.request('/v1/tunnel'),
                    client.request('/v1/security/devices'), client.request('/v1/desktop'))
        self.work(read, self.show_status, quiet=True)

    def show_status(self, values):
        status, tunnel, devices, desktop = values
        self.online = True
        self.codex_connected = status['connected']
        self.show_desktop(desktop)
        self.permission_runtime.set_text('Host 已连接 · ' + ('Codex 已连接' if self.codex_connected else 'Codex 未连接'))
        if self.tray:
            self.tray.changed()
            self.tray_status.set_text('状态图标已连接' if self.tray.available else '桌面尚未启用状态图标；关闭窗口后可从应用菜单重新打开主机。')
        if self.message.get_text() == '服务已启动，正在连接 Codex…' and status['connected']:
            self.message.set_text('服务运行中，Codex 已连接')
        owner = '本界面管理' if self.service.running else '外部 Host（只可管理连接，不能停止该进程）'
        self.status.set_text(f"Host {status['version']} · {owner}")
        runtime = status.get('runtime') or {}
        connected = 'Codex 已连接' if status['connected'] else 'Codex 未连接'
        self.runtime.set_text('\n'.join(str(v) for v in [connected, status.get('error'),
            runtime.get('mode'), runtime.get('note'), runtime.get('binary'), runtime.get('codexHome')] if v))
        self.tunnel_status.set_text('frpc：' + tunnel['state'])
        self.tunnel_log.set_text(tunnel.get('logTail', ''))
        if not self.tunnel_loaded:
            saved = tunnel.get('config') or {}
            for entry, key in ((self.server, 'server'), (self.server_port, 'serverPort'),
                               (self.remote_port, 'remotePort'), (self.ca, 'trustedCaFile')):
                if saved.get(key) is not None:
                    entry.set_text(str(saved[key]))
            if saved.get('server') and saved.get('remotePort'):
                self.endpoint.set_text(self.remote_url(saved['server'], saved['remotePort']))
            self.tunnel_loaded = True
        ids = sorted(devices['devices'])
        if ids != self.device_ids:
            while child := self.devices.get_first_child():
                self.devices.remove(child)
            self.device_ids = ids
            self.revoke_buttons = []
            if not ids:
                self.devices.append(label('暂无已配对设备', 'dim-label'))
            for device_id in ids:
                row = Gtk.Box(spacing=12)
                text = label(device_id)
                text.set_hexpand(True)
                row.append(text)
                button = Gtk.Button(label='撤销')
                button.connect('clicked', lambda _, identity=device_id: self.api('/v1/security/devices/revoke', {'deviceID': identity}))
                row.append(button)
                self.revoke_buttons.append(button)
                self.devices.append(row)
        if devices.get('pairingConsumed') and self.pair_expiry:
            self.clear_pair('配对票据已使用')
            self.pair_refresh_needed = True
        if self.permission_recheck and self.codex_connected:
            self.permission_recheck = False
            self.run_permissions(False)

    def tick(self):
        if self.closing:
            return GLib.SOURCE_REMOVE
        if self.pair_expiry and time.time() >= self.pair_expiry - 60:
            self.clear_pair('正在刷新配对票据…')
            self.pair_refresh_needed = True
        if self.pair_refresh_needed and self.stack.get_visible_child_name() == '连接设备' and not self.busy and self.online:
            self.pair_refresh_needed = False
            self.pair()
        else:
            self.refresh()
        return GLib.SOURCE_CONTINUE

    def start(self):
        self.work(self.service.start, lambda _: self.message.set_text('服务已启动，正在连接 Codex…'))

    def stop(self):
        def stopped(_):
            self.online = False
            self.codex_connected = False
            self.status.set_text('服务已停止')
            self.runtime.set_text('')
            self.clear_pair()
            self.message.set_text('由本界面启动的服务已停止')
        self.work(self.service.stop, stopped)

    def save(self):
        values = [entry.get_text().strip() for entry in (self.codex_home, self.codex_binary, self.control_socket, self.listen_port)]
        self.work(lambda: self.service.save_settings(*values))

    def start_tunnel(self):
        try:
            body = {'server': self.server.get_text().strip(), 'serverPort': int(self.server_port.get_text()),
                    'remotePort': int(self.remote_port.get_text()), 'token': self.token.get_text(),
                    'trustedCaFile': self.ca.get_text().strip() or None}
        except ValueError:
            self.message.set_text('端口必须是整数')
            return
        def done(_):
            self.token.set_text('')
            self.endpoint.set_text(self.remote_url(body['server'], body['remotePort']))
            self.message.set_text('frpc 已启动；请在远程设备验证公网连接')
        self.work(lambda: HostClient(self.service.directory).request('/v1/tunnel', body), done)

    def pair(self):
        endpoint = self.endpoint.get_text().strip()
        self.clear_pair()
        self.work(lambda: HostClient(self.service.directory).pairing_link(endpoint),
                  lambda result: self.show_pair(result) if self.endpoint.get_text().strip() == endpoint else None)

    def show_pair(self, result):
        import qrcode
        link, self.pair_expiry = result
        qr = qrcode.QRCode(border=4)
        qr.add_data(link)
        qr.make(fit=True)
        matrix = qr.get_matrix()
        scale, side = 4, len(matrix) * 4
        pixels = b''.join(b''.join((b'\x00\x00\x00' if cell else b'\xff\xff\xff') * scale
                                  for cell in row) * scale for row in matrix)
        texture = Gdk.MemoryTexture.new(side, side, Gdk.MemoryFormat.R8G8B8, GLib.Bytes.new(pixels), side * 3)
        self.qr.set_paintable(texture)
        self.qr.set_visible(True)
        self.pair_link.set_text(link)
        self.pair_link.set_visible(True)
        self.pair_status.set_text('请使用客户端扫描；二维码和链接包含一次性凭证，请勿分享。')

    def clear_pair(self, message=''):
        self.pair_refresh_needed = False
        self.pair_expiry = 0
        self.qr.set_paintable(None)
        self.qr.set_visible(False)
        self.pair_link.set_text('')
        self.pair_link.set_visible(False)
        self.pair_status.set_text(message)

    def check_access(self):
        path = self.check_path.get_text()
        self.work(lambda: self.service.check_directory(path), lambda result: self.permission_result.set_text(
            f"可读：{'是' if result['readable'] else '否'} · 可写：{'是' if result['writable'] else '否'}"
            + (f" · {result['issue']}" if result.get('issue') else '')))

    def show_log(self, text):
        self.log_view.get_buffer().set_text(text)

    def open_page(self, name):
        self.activate_window()
        self.stack.set_visible_child_name(name)

    def copy_pair_link(self):
        self.window.get_display().get_clipboard().set(self.pair_link.get_text())

    @staticmethod
    def remote_url(server, port):
        server = server.strip()
        return f"https://{'[' + server + ']' if ':' in server and not server.startswith('[') else server}:{port}"

    def automatic_update_check(self):
        if self.closing:
            return GLib.SOURCE_REMOVE
        if self.automatic_updates.get_active() and not self.busy:
            self.check_updates()
            GLib.timeout_add_seconds(86400, self.automatic_update_check)
            return GLib.SOURCE_REMOVE
        return GLib.SOURCE_CONTINUE

    def close_window(self, *_):
        self.clear_pair()
        self.window.set_visible(False)
        return True

    def request_quit(self, *_):
        if not self.closing:
            self.closing = True
            self.checks.cancelled.set()
            self.service.cancelled.set()
            if self.window:
                self.window.set_sensitive(False)
                self.message.set_text('正在停止本界面启动的服务…')
            future = self.pool.submit(self.service.stop)
            future.add_done_callback(lambda _: GLib.idle_add(self.quit))
        return True


def main():
    parser = argparse.ArgumentParser(description='AnywhereHost GTK 管理界面')
    parser.add_argument('--state-dir', type=Path, default=state_directory())
    parser.add_argument('--backend', type=Path, default=backend_binary())
    args = parser.parse_args()
    app = HostApplication(HostService(args.backend, args.state_dir))
    for signum in (signal.SIGTERM, signal.SIGINT):
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signum, lambda: app.request_quit())
    try:
        result = app.run([])
    finally:
        app.pool.shutdown(wait=True)
        app.service.stop()
        app.updater.close()
        if app.tray:
            app.tray.close()
    if app.restart_requested:
        os.execv('/usr/bin/anywhere-host-gtk', ['/usr/bin/anywhere-host-gtk'])
    return result
