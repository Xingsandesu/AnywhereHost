import AppKit
import SwiftUI

struct HostDesktopSettings: View {
    let store: HostStore
    @State private var enabled = false
    @State private var status: JSONValue = .null
    @State private var error: String?
    private var settingsURL: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appending(path: "AnywhereHost/desktop.json")
    }
    var body: some View {
        HostSheet(title: "远程桌面") {
            Form {
                Section {
                    Toggle("允许已配对设备连接", isOn: Binding(get: { enabled }, set: save))
                    LabeledContent("屏幕共享", value: status["reachable"].bool ? "可连接" : (enabled ? "不可连接" : "已关闭"))
                }
                Section {
                    Button("屏幕共享设置", systemImage: "gear") {
                        NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.Sharing-Settings.extension")!)
                    }
                    Button("刷新状态", systemImage: "arrow.clockwise") { Task { await refresh() } }
                }
                if let error { Text(error).foregroundStyle(.red).textSelection(.enabled) }
            }.formStyle(.grouped)
        }.frame(width: 480, height: 340)
        .task {
            if let data = try? Data(contentsOf: settingsURL), let settings = try? JSONDecoder().decode(JSONValue.self, from: data) {
                enabled = settings["enabled"].bool
            }
            await refresh()
        }
    }
    private func save(_ value: Bool) {
        do {
            try FileManager.default.createDirectory(at: settingsURL.deletingLastPathComponent(), withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            try JSONEncoder().encode(JSONValue.object(["enabled": .bool(value)])).write(to: settingsURL, options: .atomic)
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: settingsURL.path)
            enabled = value; error = nil
            Task { await refresh() }
        } catch { self.error = error.localizedDescription }
    }
    private func refresh() async {
        do {
            guard let client = store.client else { throw HostFailure(message: "Host 后端尚未启动") }
            status = try await client.request("v1/desktop"); error = nil
        } catch { self.error = error.localizedDescription }
    }
}
