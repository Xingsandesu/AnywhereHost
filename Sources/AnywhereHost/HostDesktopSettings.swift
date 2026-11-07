import AppKit
import SwiftUI

struct HostDesktopSettings: View {
    let store: HostStore
    @State private var enabled = false
    @State private var status: JSONValue = .null
    @State private var error: String?
    @State private var screenAllowed = false
    @State private var inputAllowed = false
    private var settingsURL: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appending(path: "AnywhereHost/desktop.json")
    }
    var body: some View {
        HostSheet(title: "远程桌面") {
            Form {
                Section {
                    Toggle("允许已配对设备连接", isOn: Binding(get: { enabled }, set: save))
                    LabeledContent("原生远程显示", value: status["nativeAvailable"].bool ? "可用" : "请重启 Host")
                    LabeledContent("屏幕录制", value: screenAllowed ? "已允许" : "需要授权")
                    LabeledContent("辅助功能", value: inputAllowed ? "已允许" : "需要授权")
                }
                Section {
                    Button("允许屏幕录制", systemImage: "record.circle") {
                        CGRequestScreenCaptureAccess()
                        store.permissions.openSettings(.screenRecording)
                    }
                    Button("允许键盘、鼠标和 Pencil 控制", systemImage: "cursorarrow") {
                        store.permissions.openSettings(.accessibility)
                    }
                    Button("刷新状态", systemImage: "arrow.clockwise") { Task { await refresh() } }
                }
                if let error { Text(error).foregroundStyle(.red).textSelection(.enabled) }
            }.formStyle(.grouped)
        }.frame(width: 480, height: 420)
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
        screenAllowed = CGPreflightScreenCaptureAccess(); inputAllowed = AXIsProcessTrusted()
        do {
            guard let client = store.client else { throw HostFailure(message: "主机服务尚未启动") }
            status = try await client.request("desktop.status"); error = nil
        } catch { self.error = error.localizedDescription }
    }
}
