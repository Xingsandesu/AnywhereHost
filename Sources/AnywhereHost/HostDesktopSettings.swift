import AppKit
import SwiftUI

struct HostDesktopSettings: View {
    let store: HostStore
    @Environment(\.dismiss) private var dismiss
    @State private var enabled = false
    @State private var status: JSONValue = .null
    @State private var error: String?
    private var settingsURL: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appending(path: "AnywhereHost/desktop.json")
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            Label("远程桌面", systemImage: "display").font(.title2.bold())
            Text("在 iPhone 的 Anywhere 中打开并控制这台 Mac。首次配置后，可从工作区直接连接。")
            Toggle("允许已配对的设备连接桌面", isOn: Binding(get: { enabled }, set: save))
            Text("关闭后，现有远程桌面连接也会断开。连接仍需要通过 Mac 屏幕共享的身份验证。")
                .font(.callout).foregroundStyle(.secondary)
            Divider()
            Text("1. 在系统设置 → 通用 → 共享中开启“屏幕共享”，选择允许访问的用户。")
            Text("2. 在 iPhone 首次连接时输入允许访问的 Mac 用户名和密码，或屏幕共享的 VNC 密码。可保存在 iPhone 钥匙串中。")
            Button("打开屏幕共享设置", systemImage: "gear") {
                NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.Sharing-Settings.extension")!)
            }
            LabeledContent("屏幕共享服务", value: status["reachable"].bool ? "可连接" : (enabled ? "未检测到，请开启后刷新" : "远程桌面已关闭"))
            Text("公网访问复用“连接 iPhone”中配置的隧道。Mac 需要保持开机、联网且 Host 正在运行。无需在路由器上开放 5900 端口。")
                .font(.callout).foregroundStyle(.secondary)
            if let error { Text(error).foregroundStyle(.red) }
            HStack {
                Button("刷新状态") { Task { await refresh() } }
                Spacer()
                Button("完成") { dismiss() }.keyboardShortcut(.defaultAction)
            }
        }.padding(28).frame(width: 540)
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
