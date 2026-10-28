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
    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            HStack { Text("连接 iPhone Duo").font(.title2.bold()); Spacer(); Button("完成") { dismiss() } }
            Text("内置 frpc 将加密的 Host 端口转发到你自己的 frps 服务器。连接信息包含访问凭证，只分享给自己的设备。")
                .foregroundStyle(.secondary)
            Form {
                Section("内网穿透") {
                    TextField("frps 地址", text: $server)
                    TextField("frps 控制端口", text: $serverPort)
                    TextField("公网服务端口", text: $remotePort)
                    SecureField("frps Token", text: $token)
                    TextField("自签 frps CA 文件路径（可选）", text: $caPath)
                    HStack {
                        Label(store.tunnel["state"].string ?? "stopped", systemImage: "network")
                        Spacer()
                        Button("启动隧道") {
                            Task { await store.perform {
                                guard let control = UInt16(serverPort), control > 0, let remote = UInt16(remotePort), remote > 0 else { throw HostFailure(message: "端口应为 1–65535") }
                                _ = try await store.client?.request("v1/tunnel", body: .object([
                                    "server": .string(server), "serverPort": .number(Double(control)), "remotePort": .number(Double(remote)), "token": .string(token), "trustedCaFile": caPath.isEmpty ? .null : .string(caPath)
                                ]))
                                endpoint = "https://\(server):\(remote)"; await store.refresh()
                            } }
                        }.disabled(store.busy || server.isEmpty || token.isEmpty)
                        Button("停止") { Task { await store.perform { _ = try await store.client?.request("v1/tunnel/stop", body: .object([:])); await store.refresh() } } }.disabled(store.busy)
                    }
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
                        Text("包含 HTTPS 地址、访问令牌和证书指纹。旧版 cc-connect 客户端暂不识别此链接。").foregroundStyle(.secondary)
                        Button("复制连接链接") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(link, forType: .string) }
                    }
                }
            }
            HStack {
                Button("打开主机日志") { NSWorkspace.shared.open(store.process.stateDirectory.appending(path: "host.log")) }
                Spacer()
                Text("默认只监听本机 · TLS 证书固定验证").font(.caption).foregroundStyle(.secondary)
            }
        }.padding(24).frame(width: 660, height: qr == nil ? 580 : 790)
    }
    private static func makeQR(_ text: String) -> NSImage? {
        let filter = CIFilter.qrCodeGenerator(); filter.message = Data(text.utf8); filter.correctionLevel = "M"
        guard let output = filter.outputImage, let cg = CIContext().createCGImage(output, from: output.extent) else { return nil }
        return NSImage(cgImage: cg, size: NSSize(width: cg.width, height: cg.height))
    }
}
