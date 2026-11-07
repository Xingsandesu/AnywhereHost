import AppKit
import SwiftUI

struct HostDesktopSettings: View {
    let store: HostStore
    @State private var enabled = false
    @State private var status: JSONValue = .null
    @State private var error: String?
    @State private var screenAllowed = false
    @State private var inputAllowed = false
    @State private var touchEnabled = DesktopTouchAuthorization.isEnabled
    @State private var checkingTouch = false
    @State private var touchStatus = "尚未检测"
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
                Section("原生多点触控（实验）") {
                    LabeledContent("虚拟触屏", value: touchEnabled ? "本机已允许" : "未启用")
                    Text(touchStatus).foregroundStyle(.secondary)
                    Button(checkingTouch ? "正在检测…" : "检测并启用虚拟触屏") {
                        Task { await enableNativeTouch() }
                    }.disabled(checkingTouch)
                    if touchEnabled {
                        Button("撤销原生触控授权", role: .destructive) {
                            do { try DesktopTouchAuthorization.save(false); touchEnabled = false; touchStatus = "已撤销" }
                            catch { self.error = error.localizedDescription }
                        }
                    }
                    Text("将手指作为触屏设备传给 macOS。需要具备虚拟 HID 权限的 Host 签名，启用前将打开独立窗口验证系统是否收到多个直接触点。")
                        .font(.caption).foregroundStyle(.secondary)
                }
                if let error { Text(error).foregroundStyle(.red).textSelection(.enabled) }
            }.formStyle(.grouped)
        }.frame(width: 520, height: 620)
        .task {
            if let data = try? Data(contentsOf: settingsURL), let settings = try? JSONDecoder().decode(JSONValue.self, from: data) {
                enabled = settings["enabled"].bool
            }
            await refresh()
        }
    }
    private func enableNativeTouch() async {
        checkingTouch = true
        defer { checkingTouch = false }
        do {
            try await DesktopNativeTouchProbe.verify()
            try DesktopTouchAuthorization.save(true)
            touchEnabled = true; touchStatus = "已验证 AppKit 接收多个直接触点"; error = nil
        } catch { touchStatus = "检测未通过"; self.error = error.localizedDescription }
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
