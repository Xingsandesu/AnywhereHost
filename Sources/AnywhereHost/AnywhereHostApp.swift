import AppKit
import SwiftUI
import CryptoKit

@MainActor final class AppDelegate: NSObject, NSApplicationDelegate {
    let store = HostStore()
    let updater = HostUpdater()
    func applicationDidFinishLaunching(_ notification: Notification) {
        if ProcessInfo.processInfo.arguments.contains("--verify-network-admission") {
            NSApp.setActivationPolicy(.prohibited)
            Task {
                do {
                    let lease = try await NetworkAdmission(role: "host").lease(secret: Curve25519.Signing.PrivateKey().rawRepresentation)
                    FileHandle.standardOutput.write(Data("PASS production Host App Attest and network lease; expires=\(lease.expiresAt)\n".utf8))
                    exit(0)
                } catch {
                    FileHandle.standardError.write(Data("Host admission verification failed: \(error.localizedDescription)\n".utf8))
                    exit(1)
                }
            }
            return
        }
        NSApp.setActivationPolicy(.regular); NSApp.activate()
    }
    func applicationWillTerminate(_ notification: Notification) { store.stop() }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
}
struct AnywhereHostApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    var body: some Scene {
        Window("Anywhere Host", id: "host") {
            if ProcessInfo.processInfo.arguments.contains("--verify-network-admission") {
                EmptyView()
            } else {
                HostSetupRootView(store: delegate.store)
                    .frame(minWidth: 600, maxWidth: 800, minHeight: 600, maxHeight: 800)
            }
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
