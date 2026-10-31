import AppKit
import SwiftUI

@MainActor final class AppDelegate: NSObject, NSApplicationDelegate {
    let store = HostStore()
    let updater = HostUpdater()
    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular); NSApp.activate()
    }
    func applicationWillTerminate(_ notification: Notification) { store.stop() }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
}
@main struct AnywhereHostApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    var body: some Scene {
        Window("Anywhere Host", id: "host") {
            HostSetupRootView(store: delegate.store)
                .frame(minWidth: 600, maxWidth: 800, minHeight: 600, maxHeight: 800)
        }
        .defaultSize(width: 680, height: 640)
        .windowResizability(.contentSize)
        .commands {
            CommandGroup(replacing: .newItem) {}
            CommandGroup(after: .appInfo) { HostUpdateButton(updater: delegate.updater) }
        }
        MenuBarExtra("Anywhere Host", systemImage: "desktopcomputer") {
            HostMenu(store: delegate.store, updater: delegate.updater)
        }
    }
}

struct HostMenu: View {
    var store: HostStore
    let updater: HostUpdater
    @Environment(\.openWindow) private var openWindow
    var body: some View {
        Text(store.serviceAvailable ? "主机运行中" : "主机未连接")
        Button("打开主机") { openWindow(id: "host"); NSApp.activate() }
        Button("权限…") {
            openWindow(id: "host"); NSApp.activate()
            store.permissions.openPermissions()
        }
        Button("设置向导…") {
            openWindow(id: "host"); NSApp.activate()
            store.permissions.openOnboarding()
        }
        Divider()
        Text("版本 \(HostVersion.display)")
        HostUpdateButton(updater: updater)
        Button("退出 Anywhere Host") { NSApp.terminate(nil) }
    }
}
