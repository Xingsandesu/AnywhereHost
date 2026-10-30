import AppKit
import SwiftUI

@MainActor final class AppDelegate: NSObject, NSApplicationDelegate {
    let store = HostStore()
    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular); NSApp.activate(ignoringOtherApps: true)
        if let flag = CommandLine.arguments.firstIndex(of: "--snapshot"), CommandLine.arguments.count > flag + 1 {
            let path = CommandLine.arguments[flag + 1]
            Task {
                for _ in 0..<40 {
                    try? await Task.sleep(for: .milliseconds(250))
                    if store.connected { break }
                }
                try? await Task.sleep(for: .milliseconds(500))
                guard let view = NSApp.windows.first(where: { $0.contentView != nil && $0.frame.width > 600 })?.contentView,
                      let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
                view.effectiveAppearance.performAsCurrentDrawingAppearance {
                    view.cacheDisplay(in: view.bounds, to: bitmap)
                }
                if let png = bitmap.representation(using: .png, properties: [:]) { try? png.write(to: URL(fileURLWithPath: path)) }
            }
        }
    }
    func applicationWillTerminate(_ notification: Notification) { store.stop() }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
}
@main struct AnywhereHostApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    var body: some Scene {
        Window("Anywhere Host", id: "host") {
            HostSetupRootView(store: delegate.store)
                .frame(minWidth: 960, minHeight: 640)
        }
        .defaultSize(width: 1180, height: 780)
        .commands { CommandGroup(replacing: .newItem) {} }
        MenuBarExtra("Anywhere Host", systemImage: "laptopcomputer.and.iphone") {
            HostMenu(store: delegate.store)
        }
    }
}

struct HostMenu: View {
    var store: HostStore
    @Environment(\.openWindow) private var openWindow
    var body: some View {
        Text(store.runtimeLabel)
        Button("显示主机控制台") { openWindow(id: "host"); NSApp.activate(ignoringOtherApps: true) }
        Button("权限检查…") {
            openWindow(id: "host"); NSApp.activate(ignoringOtherApps: true)
            store.permissions.openPermissions()
        }
        Button("重新查看使用引导…") {
            openWindow(id: "host"); NSApp.activate(ignoringOtherApps: true)
            store.permissions.openOnboarding()
        }
        Divider()
        Button("退出 Anywhere Host") { NSApp.terminate(nil) }
    }
}
