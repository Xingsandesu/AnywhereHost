"""StatusNotifierItem + DBusMenu, without mixing GTK 3 and GTK 4."""
from gi.repository import Gio, GLib

ITEM_XML = '''<node><interface name="org.kde.StatusNotifierItem">
<method name="Activate"><arg type="i" direction="in"/><arg type="i" direction="in"/></method>
<method name="SecondaryActivate"><arg type="i" direction="in"/><arg type="i" direction="in"/></method>
<method name="ContextMenu"><arg type="i" direction="in"/><arg type="i" direction="in"/></method>
<method name="Scroll"><arg type="i" direction="in"/><arg type="s" direction="in"/></method>
<property name="Category" type="s" access="read"/><property name="Id" type="s" access="read"/>
<property name="Title" type="s" access="read"/><property name="Status" type="s" access="read"/>
<property name="IconName" type="s" access="read"/><property name="IconPixmap" type="a(iiay)" access="read"/>
<property name="ToolTip" type="(sa(iiay)ss)" access="read"/>
<property name="Menu" type="o" access="read"/><property name="ItemIsMenu" type="b" access="read"/>
<signal name="NewStatus"><arg type="s"/></signal><signal name="NewToolTip"/>
</interface></node>'''
MENU_XML = '''<node><interface name="com.canonical.dbusmenu">
<method name="GetLayout"><arg type="i" direction="in"/><arg type="i" direction="in"/><arg type="as" direction="in"/>
<arg type="u" direction="out"/><arg type="(ia{sv}av)" direction="out"/></method>
<method name="GetGroupProperties"><arg type="ai" direction="in"/><arg type="as" direction="in"/><arg type="a(ia{sv})" direction="out"/></method>
<method name="GetProperty"><arg type="i" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="out"/></method>
<method name="Event"><arg type="i" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="in"/><arg type="u" direction="in"/></method>
<method name="AboutToShow"><arg type="i" direction="in"/><arg type="b" direction="out"/></method>
<property name="Version" type="u" access="read"/><property name="TextDirection" type="s" access="read"/>
<property name="Status" type="s" access="read"/>
</interface></node>'''


class Tray:
    def __init__(self, application):
        self.app = application
        self.available = False
        self.connection = Gio.bus_get_sync(Gio.BusType.SESSION, None)
        self.menu = [('打开主机', 'open'), ('权限…', 'permissions'), ('设置向导…', 'setup'),
                     ('检查更新…', 'updates'), ('退出 AnywhereHost', 'quit')]
        self.registrations = [self.connection.register_object('/StatusNotifierItem',
            Gio.DBusNodeInfo.new_for_xml(ITEM_XML).interfaces[0], self.item_call, self.item_property, None),
            self.connection.register_object('/Menu', Gio.DBusNodeInfo.new_for_xml(MENU_XML).interfaces[0],
                                            self.menu_call, self.menu_property, None)]
        self.watcher = Gio.bus_watch_name_on_connection(self.connection, 'org.kde.StatusNotifierWatcher',
            Gio.BusNameWatcherFlags.NONE, self.register, self.disappeared)

    def register(self, *_):
        self.connection.call('org.kde.StatusNotifierWatcher', '/StatusNotifierWatcher',
            'org.kde.StatusNotifierWatcher', 'RegisterStatusNotifierItem',
            GLib.Variant('(s)', (self.connection.get_unique_name(),)), None,
            Gio.DBusCallFlags.NONE, 3000, None, self.registered, None)

    def registered(self, connection, result, _):
        try:
            connection.call_finish(result)
            self.available = True
        except GLib.Error:
            pass  # The launcher and application menu remain available without a tray host.

    def disappeared(self, *_):
        self.available = False

    def item_property(self, _c, _s, _p, _i, name):
        properties = {'Category': ('s', 'ApplicationStatus'), 'Id': ('s', 'AnywhereHost'),
            'Title': ('s', 'AnywhereHost'), 'Status': ('s', 'Active'),
            'IconName': ('s', 'com.jintongshu.AnywhereHost'), 'IconPixmap': ('a(iiay)', []),
            'ToolTip': ('(sa(iiay)ss)', ('com.jintongshu.AnywhereHost', [], 'AnywhereHost',
                '主机运行中' if self.app.online else '主机未连接')),
            'Menu': ('o', '/Menu'), 'ItemIsMenu': ('b', False)}
        return GLib.Variant(*properties[name])

    def item_call(self, _c, _s, _p, _i, method, _args, invocation):
        if method in ('Activate', 'SecondaryActivate', 'ContextMenu'):
            self.app.activate_action('open', None)
        invocation.return_value(None)

    def properties(self, identity):
        if identity == 0:
            return {'children-display': GLib.Variant('s', 'submenu')}
        return {'label': GLib.Variant('s', self.menu[identity - 1][0]),
                'enabled': GLib.Variant('b', True), 'visible': GLib.Variant('b', True)}

    def node(self, identity, depth):
        children = [GLib.Variant('(ia{sv}av)', self.node(index, 0)) for index in range(1, len(self.menu) + 1)] if identity == 0 and depth != 0 else []
        return identity, self.properties(identity), children

    def menu_property(self, _c, _s, _p, _i, name):
        return GLib.Variant(*{'Version': ('u', 3), 'TextDirection': ('s', 'ltr'), 'Status': ('s', 'normal')}[name])

    def menu_call(self, _c, _s, _p, _i, method, args, invocation):
        values = args.unpack()
        try:
            if method == 'GetLayout':
                result = GLib.Variant('(u(ia{sv}av))', (1, self.node(values[0], values[1])))
            elif method == 'GetGroupProperties':
                result = GLib.Variant('(a(ia{sv}))', ([(i, self.properties(i)) for i in values[0]],))
            elif method == 'GetProperty':
                result = GLib.Variant('(v)', (self.properties(values[0])[values[1]],))
            elif method == 'AboutToShow':
                result = GLib.Variant('(b)', (False,))
            else:
                if method == 'Event' and values[1] == 'clicked' and 1 <= values[0] <= len(self.menu):
                    self.app.activate_action(self.menu[values[0] - 1][1], None)
                result = None
            invocation.return_value(result)
        except (IndexError, KeyError):
            invocation.return_dbus_error('com.canonical.dbusmenu.InvalidMenuItem', 'Unknown menu item')

    def changed(self):
        self.connection.emit_signal(None, '/StatusNotifierItem', 'org.kde.StatusNotifierItem', 'NewToolTip', None)

    def close(self):
        Gio.bus_unwatch_name(self.watcher)
        for registration in self.registrations:
            self.connection.unregister_object(registration)
