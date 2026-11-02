"""Desktop, permissions, setup and update panels for the GTK shell."""
from pathlib import Path
import time
from gi.repository import Gio, GLib, Gtk


def text(value=''):
    return Gtk.Label(label=value, xalign=0, wrap=True, selectable=True)


class ManagementPanels:
    def enable_tray(self):
        def operation():
            connection = Gio.bus_get_sync(Gio.BusType.SESSION, None)
            result = connection.call_sync('org.gnome.Shell.Extensions', '/org/gnome/Shell/Extensions',
                'org.gnome.Shell.Extensions', 'EnableExtension', GLib.Variant('(s)', ('ubuntu-appindicators@ubuntu.com',)),
                GLib.VariantType.new('(b)'), Gio.DBusCallFlags.NONE, 5000, None)
            if not result.unpack()[0]:
                raise RuntimeError('请安装 gnome-shell-extension-appindicator 并重新登录桌面后启用')
        self.work(operation)

    def build_panels(self):
        page = self.page('远程桌面', '允许已配对设备访问本机桌面；仍需独立桌面密码。Wayland 屏幕与输入由系统授权。')
        self.desktop_status = text('正在检查桌面共享能力…')
        page.append(self.desktop_status)
        self.desktop_password = self.field(page, '独立桌面密码（8 位 ASCII；已设置时留空）', secret=True)
        row = Gtk.Box(spacing=10)
        self.desktop_enable = self.button('允许已配对设备连接', lambda: self.save_desktop(True), True)
        self.desktop_disable = self.button('关闭桌面共享', lambda: self.save_desktop(False))
        row.append(self.desktop_enable)
        row.append(self.desktop_disable)
        page.append(row)
        page.append(self.button('授权并检查屏幕与输入', self.authorize_desktop))
        self.desktop_authorization = text('尚未验证屏幕与输入授权')
        page.append(self.desktop_authorization)
        page.append(self.button('系统隐私设置', lambda: self.launch_settings('privacy')))
        page.append(self.button('打开桌面日志', lambda: self.open_file(self.service.directory / 'desktop.log')))
        page.append(text('首次连接时，在本机系统弹窗中选择屏幕并允许键盘、鼠标控制。撤销设备或关闭共享会断开对应连接。'))
        page.append(text('屏幕共享不监听公网或局域网端口，客户端通过现有 Host HTTPS/WSS 隧道连接。'))

        page = self.page('权限', '验证 Host 实际目录访问；Linux 屏幕与输入访问由桌面门户单独授权。')
        row = Gtk.Box(spacing=10)
        row.append(self.button('检查全部权限', self.run_permissions, True))
        row.append(self.button('添加目录', self.choose_directory))
        cancel = Gtk.Button(label='停止检查')
        cancel.connect('clicked', lambda _: self.checks.cancelled.set())
        row.append(cancel)
        page.append(row)
        self.permission_rows = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        page.append(self.permission_rows)
        self.permission_summary = text('尚未检查')
        page.append(self.permission_summary)
        page.append(text('自动化：沿用当前 Linux 用户权限和 Codex 原生审批；屏幕与输入通过桌面门户授权。'))
        page.append(text('目录检查不等于共享 Codex 进程已获授权；不存在 macOS 的“完全磁盘访问”授权状态。'))
        for title, arguments in [('文件与文件夹设置', ('privacy',)), ('屏幕与输入授权设置', ('privacy',)),
                                 ('本地网络设置', ('network',))]:
            page.append(self.button(title, lambda arguments=arguments: self.launch_settings(*arguments)))
        page.append(self.button('在文件管理器中显示应用', lambda: self.open_file(Path(__file__).resolve().parent)))
        self.permission_runtime = text('')
        page.append(self.permission_runtime)

        page = self.page('更新', '检查、下载并验证 Ed25519 签名，安装经系统授权后完成。用户配置与设备配对保留。')
        self.update_status = text('尚未检查更新')
        page.append(self.update_status)
        self.automatic_updates = Gtk.CheckButton(label='自动检查更新')
        self.automatic_updates.set_active(self.preferences.get('automaticUpdates', True))
        self.automatic_updates.connect('toggled', lambda button: self.preferences.set('automaticUpdates', button.get_active()))
        page.append(self.automatic_updates)
        page.append(self.button('检查更新…', self.check_updates))
        self.download_button = self.button('下载并验证', self.download_update)
        self.install_button = self.button('安装并重新启动', self.install_update)
        page.append(self.download_button)
        page.append(self.install_button)
        self.last_update_check = time.monotonic()
        self.update_status.set_text('自动检查已启用' if self.automatic_updates.get_active() else '自动检查已关闭')

    def open_file(self, path):
        try:
            Gio.AppInfo.launch_default_for_uri(Path(path).resolve().as_uri(), None)
        except GLib.Error as error:
            self.message.set_text(str(error))

    def launch_settings(self, *arguments):
        import subprocess
        try:
            subprocess.Popen(['gnome-control-center', *arguments], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        except OSError as error:
            self.message.set_text(f'无法打开系统设置：{error}')

    def save_desktop(self, enabled):
        password = self.desktop_password.get_text()
        def done(_):
            self.desktop_password.set_text('')
            self.message.set_text('桌面共享已启用' if enabled else '桌面共享已关闭')
        self.work(lambda: self.service.set_desktop(enabled, password if enabled else ''), done)

    def authorize_desktop(self):
        from .desktop import authorize
        password = self.desktop_password.get_text()
        self.desktop_password.set_text('')
        self.desktop_authorization.set_text('请在本机系统弹窗中选择屏幕并允许键盘、鼠标控制…')
        self.work(lambda: authorize(self.service.directory, password, self.service.cancelled),
                  self.desktop_authorization.set_text)

    def show_desktop(self, value):
        self.desktop_supported = value['supported']
        self.desktop_enabled = value['enabled']
        platform = value.get('platform', {})
        state = '已关闭' if not value['enabled'] else ('可连接' if value['reachable'] else '正在启动或等待授权')
        self.desktop_status.set_text('\n'.join(str(part) for part in [
            '屏幕共享：' + state, f"活动连接：{value['activeSessions']} / 2", platform.get('helper'),
            platform.get('error'), value.get('reason')] if part))

    def choose_directory(self):
        dialog = Gtk.FileDialog(title='选择实际使用的项目目录')
        def selected(dialog, result):
            try:
                path = dialog.select_folder_finish(result).get_path()
                if path:
                    self.preferences.set('permissionDirectory', path)
                    self.run_permissions()
            except GLib.Error:
                pass
        dialog.select_folder(self.window, None, selected)

    def run_permissions(self, optional=True):
        self.work(lambda: self.checks.run(optional), self.show_permissions)

    def show_permissions(self, results):
        while child := self.permission_rows.get_first_child():
            self.permission_rows.remove(child)
        for title, path, required, result in results:
            state = '可读写' if result['readable'] and result['writable'] else '仅可读' if result['readable'] else result.get('issue') or '无法访问'
            self.permission_rows.append(text(f"{title}{'（必需）' if required else ''} · {state}\n{path}"))
        passed = sum(result['readable'] and result['writable'] for _, _, _, result in results)
        self.permission_summary.set_text(f'{passed} / {len(results)} 项可读写' if results else '检查已取消')
        self.update_setup()

    def open_setup(self):
        if getattr(self, 'setup_window', None):
            self.setup_window.present()
            return
        self.setup_step = 0
        self.setup_window = Gtk.Window(title='设置向导', transient_for=self.window, modal=True, default_width=560, default_height=420)
        self.setup_window.set_hide_on_close(True)
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=24)
        for edge in ('top', 'bottom', 'start', 'end'):
            getattr(box, f'set_margin_{edge}')(24)
        self.setup_title = text('欢迎 · 1 / 3')
        self.setup_title.add_css_class('title-1')
        self.setup_body = text('将这台设备配置为 AnywhereHost。')
        self.setup_body.set_vexpand(True)
        box.append(self.setup_title)
        box.append(self.setup_body)
        self.setup_check = Gtk.Button(label='检查必需目录权限')
        self.setup_check.connect('clicked', lambda _: self.run_permissions(False))
        box.append(self.setup_check)
        row = Gtk.Box(spacing=10)
        skip = Gtk.Button(label='稍后设置')
        skip.connect('clicked', lambda _: self.finish_setup(False))
        self.setup_back = Gtk.Button(label='上一步')
        self.setup_back.connect('clicked', lambda _: self.setup_move(-1))
        self.setup_next = Gtk.Button(label='开始设置')
        self.setup_next.add_css_class('suggested-action')
        self.setup_next.connect('clicked', lambda _: self.setup_move(1))
        for button in (skip, self.setup_back, self.setup_next):
            row.append(button)
        box.append(row)
        self.setup_window.set_child(box)
        self.setup_window.connect('close-request', lambda *_: self.finish_setup(False))
        self.update_setup()
        self.setup_window.present()

    def update_setup(self):
        if not getattr(self, 'setup_window', None):
            return
        ready = self.checks.ready and self.online and self.codex_connected and not self.busy
        if self.setup_step == 2 and not ready:
            self.setup_step = 1
        self.setup_title.set_text(['欢迎 · 1 / 3', '权限 · 2 / 3', '就绪 · 3 / 3'][self.setup_step])
        self.setup_body.set_text(['AnywhereHost\n安全连接你的 Codex 工作主机。',
            '检查主机数据、实际 Codex Home 和所选目录的读写权限。\n' + self.permission_summary.get_text()
            + ('\nCodex 已连接' if self.codex_connected else '\n等待 Codex 连接；可返回主界面设置运行时。'),
            '目录权限与 Codex 连接均已就绪。'][self.setup_step])
        self.setup_check.set_visible(self.setup_step == 1)
        self.setup_check.set_sensitive(not self.busy)
        self.setup_back.set_sensitive(self.setup_step > 0 and not self.busy)
        self.setup_next.set_label(['开始设置', '继续', '完成'][self.setup_step])
        self.setup_next.set_sensitive(not self.busy and (self.setup_step == 0 or ready))

    def setup_move(self, delta):
        if self.setup_step == 2 and delta > 0:
            self.finish_setup(True)
            return
        self.setup_step += delta
        if self.setup_step == 1 and not self.service.running and not self.online:
            self.start()
        self.update_setup()

    def finish_setup(self, complete):
        if complete and not (self.checks.ready and self.online and self.codex_connected):
            return True
        if complete:
            self.preferences.set('onboardingVersion', 1)
        window = self.setup_window
        self.setup_window = None
        window.destroy()
        if not self.service.running and not self.online and not self.busy:
            self.start()
        return True

    def check_updates(self):
        self.last_update_check = time.monotonic()
        self.work(self.updater.check, self.update_status.set_text)

    def download_update(self):
        self.work(self.updater.fetch, self.update_status.set_text)

    def install_update(self):
        def operation():
            running = self.service.running
            self.service.stop()
            try:
                self.updater.install()
            except Exception:
                if running:
                    self.service.start()
                raise
        def done(_):
            self.restart_requested = True
            self.request_quit()
        self.work(operation, done)
