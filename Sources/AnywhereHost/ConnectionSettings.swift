import AppKit
import SwiftUI
import CoreImage.CIFilterBuiltins

struct ConnectionSettings: View {
    @Bindable var store: HostStore
    @Environment(\.dismiss) private var dismiss
    @State private var server = ""
    @State private var serverPort = "7000"
    @State private var remotePort = "9443"
    @State private var token = ""
    @State private var caPath = ""
    @State private var endpoint = ""
    @State private var link: String?
    @State private var qr: NSImage?
    @State private var loading = true
    private var canReuseToken: Bool {
        let saved = store.tunnel["config"]
        return saved["hasToken"].bool && saved["server"].string == server.trimmingCharacters(in: .whitespacesAndNewlines)
            && Self.portText(saved["serverPort"]) == serverPort
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            HStack { Text("连接 iPhone Duo").font(.title2.bold()); Spacer(); Button("完成") { dismiss() } }
            Text("内置 frpc 将加密的 Host 端口转发到你自己的 frps 服务器。连接信息包含访问凭证，只分享给自己的设备。")
                .foregroundStyle(.secondary)
            Form {
                Section("内网穿透") {
                    TextField("frps 地址", text: $server)
                    TextField("frps 控制端口（serverPort）", text: $serverPort)
                    TextField("Host 映射端口（remotePort）", text: $remotePort)
                    Text("控制端口填写服务商给出的 serverPort；映射端口是 iPhone 访问 Host 的端口，必须另外选择并在服务器上放行。")
                        .font(.caption).foregroundStyle(.secondary)
                    SecureField("frps Token", text: $token, prompt: Text(canReuseToken ? "已保存，留空继续使用" : "输入服务器 token"))
                    TextField("frps CA 文件路径（可选）", text: $caPath)
                    Text("CA 留空使用 frp 默认 TLS 加密，不验证 frps 证书。填写 CA 可验证服务器身份；Host 的 HTTPS 始终校验证书指纹。")
                        .font(.caption).foregroundStyle(.secondary)
                    HStack {
                        TunnelStateLabel(state: store.tunnel["state"].string)
                        Spacer()
                        Button("启动隧道") {
                            Task { await store.perform {
                                guard let control = UInt16(serverPort), control > 0, let remote = UInt16(remotePort), remote > 0 else { throw HostFailure(message: "端口应为 1–65535") }
                                guard let client = store.client else { throw HostFailure(message: "主机服务尚未连接") }
                                store.error = nil
                                do {
                                    _ = try await client.request("v1/tunnel", body: .object([
                                    "server": .string(server), "serverPort": .number(Double(control)), "remotePort": .number(Double(remote)), "token": .string(token), "trustedCaFile": caPath.isEmpty ? .null : .string(caPath)
                                ]))
                                } catch {
                                    await store.refresh()
                                    throw error
                                }
                                endpoint = Self.remoteURL(server: server, port: remote)
                                token = ""
                                await store.refresh()
                            } }
                        }.disabled(loading || store.busy || server.isEmpty || (token.isEmpty && !canReuseToken) || store.tunnel["state"].string == "running")
                        Button("停止") { Task { await store.perform { _ = try await store.client?.request("v1/tunnel/stop", body: .object([:])); await store.refresh() } } }.disabled(store.busy)
                    }
                }
                Section("连接诊断") {
                    TunnelDiagnostics(log: store.tunnel["logTail"].string ?? "", error: store.error)
                }
                Section("设备连接") {
                    TextField("HTTPS 远程地址", text: $endpoint, prompt: Text("https://your-host:9443"))
                    Button("生成连接二维码") {
                        do {
                            guard let client = store.client else { return }
                            let result = try client.pairingLink(endpoint: endpoint)
                            link = result; qr = Self.makeQR(result)
                        } catch { store.error = error.localizedDescription }
                    }
                }
            }.formStyle(.grouped)
            if let qr, let link {
                HStack(spacing: 20) {
                    Image(nsImage: qr).interpolation(.none).resizable().scaledToFit().frame(width: 180, height: 180).padding(10).background(.white)
                    VStack(alignment: .leading, spacing: 12) {
                        Text("Anywhere Host · 协议 v1").font(.headline)
                        Text("包含 HTTPS 地址、访问令牌和证书指纹。请在 iPhone 的 Anywhere 中导入。").foregroundStyle(.secondary)
                        Button("复制连接链接") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(link, forType: .string) }
                    }
                }
            }
            HStack {
                Button("打开隧道日志") { NSWorkspace.shared.open(store.process.stateDirectory.appending(path: "frpc.log")) }
                Spacer()
                Text("默认只监听本机 · TLS 证书固定验证").font(.caption).foregroundStyle(.secondary)
            }
        }.padding(24).frame(width: 720, height: qr == nil ? 680 : 870)
        .task {
            defer { loading = false }
            guard let client = store.client else { return }
            do {
                store.tunnel = try await client.request("v1/tunnel")
                let saved = store.tunnel["config"]
                guard let address = saved["server"].string else { return }
                server = address
                serverPort = Self.portText(saved["serverPort"]) ?? "7000"
                remotePort = Self.portText(saved["remotePort"]) ?? "9443"
                caPath = saved["trustedCaFile"].string ?? ""
                if let port = UInt16(remotePort) { endpoint = Self.remoteURL(server: address, port: port) }
            } catch { store.error = error.localizedDescription }
        }
    }
    private static func portText(_ value: JSONValue) -> String? {
        if case .number(let port) = value, let port = UInt16(exactly: port) { return String(port) }
        return nil
    }
    private static func remoteURL(server: String, port: UInt16) -> String {
        var url = URLComponents()
        url.scheme = "https"; url.host = server.trimmingCharacters(in: .whitespacesAndNewlines); url.port = Int(port)
        return url.url?.absoluteString ?? ""
    }
    private static func makeQR(_ text: String) -> NSImage? {
        let filter = CIFilter.qrCodeGenerator(); filter.message = Data(text.utf8); filter.correctionLevel = "M"
        guard let output = filter.outputImage, let cg = CIContext().createCGImage(output, from: output.extent) else { return nil }
        return NSImage(cgImage: cg, size: NSSize(width: cg.width, height: cg.height))
    }
}


private struct TunnelStateLabel: View {
    let state: String?
    var body: some View {
        switch state {
        case "running": Label("frpc 运行中", systemImage: "network")
        case "exited": Label("连接已退出", systemImage: "exclamationmark.triangle")
        case "unknown": Label("状态未知", systemImage: "questionmark.circle")
        default: Label("未启动", systemImage: "network.slash")
        }
    }
}

private struct TunnelDiagnostics: View {
    let log: String
    let error: String?
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let error { Text(error).foregroundStyle(.red).textSelection(.enabled) }
            if log.isEmpty {
                Text("启动后会显示认证和端口映射结果。").foregroundStyle(.secondary)
            } else {
                Text(verbatim: log).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
            }
        }
    }
}
