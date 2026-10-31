import AppKit
import SwiftUI
import CoreImage.CIFilterBuiltins

struct ConnectionSettings: View {
    @Bindable var store: HostStore
    @State private var server = ""
    @State private var serverPort = "7000"
    @State private var remotePort = "9443"
    @State private var token = ""
    @State private var caPath = ""
    @State private var endpoint = ""
    @State private var link: String?
    @State private var qr: NSImage?
    @State private var loading = true
    @State private var deviceIDs: [String] = []
    private var canReuseToken: Bool {
        let saved = store.tunnel["config"]
        return saved["hasToken"].bool && saved["server"].string == server.trimmingCharacters(in: .whitespacesAndNewlines)
            && Self.portText(saved["serverPort"]) == serverPort
    }
    var body: some View {
        HostSheet(title: "连接设备") {
            Form {
                Section("隧道") {
                    TextField("服务器", text: $server, prompt: Text("frps.example.com"))
                    TextField("控制端口", text: $serverPort)
                    TextField("映射端口", text: $remotePort)
                    SecureField("访问令牌", text: $token, prompt: Text(canReuseToken ? "已保存" : "Token"))
                    TextField("CA 证书", text: $caPath, prompt: Text("可选"))
                    LabeledContent("状态") { TunnelStateLabel(state: store.tunnel["state"].string) }
                    HStack {
                        Button("启动隧道", systemImage: "network") { startTunnel() }
                            .disabled(loading || store.client == nil || store.busy || server.isEmpty || (token.isEmpty && !canReuseToken) || store.tunnel["state"].string == "running")
                        Button("停止") {
                            Task { await store.perform {
                                _ = try await store.client?.request("v1/tunnel/stop", body: .object([:]))
                                await store.refresh()
                            } }
                        }.disabled(store.busy || store.client == nil || store.tunnel["state"].string != "running")
                    }
                }
                Section("配对") {
                    TextField("远程地址", text: $endpoint, prompt: Text("https://your-host:9443"))
                    Button("生成二维码", systemImage: "qrcode") {
                        Task { await generatePairing() }
                    }.disabled(store.client == nil || endpoint.isEmpty || store.busy)
                    if let qr, let link {
                        VStack(spacing: 16) {
                            Image(nsImage: qr).interpolation(.none).resizable().scaledToFit()
                                .frame(width: 180, height: 180).padding(12)
                                .background(.white, in: .rect(cornerRadius: 16))
                                .accessibilityLabel("设备配对二维码")
                            Button("复制连接链接", systemImage: "link") {
                                NSPasteboard.general.clearContents(); NSPasteboard.general.setString(link, forType: .string)
                            }
                        }.frame(maxWidth: .infinity).padding(.vertical, 12)
                    }
                }
                if !deviceIDs.isEmpty {
                    Section("已配对设备") {
                        ForEach(deviceIDs, id: \.self) { id in
                            HStack {
                                Text("设备 \(id.prefix(12))").font(.caption.monospaced())
                                Spacer()
                                Button("撤销连接", role: .destructive) {
                                    Task { await store.perform {
                                        _ = try await store.client?.request("v1/security/devices/revoke", body: .object(["deviceID": .string(id)]))
                                        await loadDevices()
                                    } }
                                }.disabled(store.busy)
                            }
                        }
                    }
                }
                Section {
                    DisclosureGroup("连接诊断") {
                        TunnelDiagnostics(log: store.tunnel["logTail"].string ?? "")
                        Button("打开日志", systemImage: "doc.text") {
                            NSWorkspace.shared.open(store.process.stateDirectory.appending(path: "frpc.log"))
                        }
                    }
                }
                if let error = store.error {
                    Text(error).foregroundStyle(.red).textSelection(.enabled)
                }
            }.formStyle(.grouped)
        }
        .frame(width: 560, height: 640)
        .task {
            while !Task.isCancelled {
                await loadDevices()
                do { try await Task.sleep(for: .seconds(5)) } catch { return }
            }
        }
        .task(id: link) {
            guard link != nil else { return }
            do { try await Task.sleep(for: .seconds(240)) } catch { return }
            await generatePairing()
        }
        .onChange(of: endpoint) { _, _ in link = nil; qr = nil }
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
    private func generatePairing() async {
        await store.perform {
            guard let client = store.client else { return }
            let requestedEndpoint = endpoint
            let result = try await client.pairingLink(endpoint: requestedEndpoint)
            guard !Task.isCancelled, endpoint == requestedEndpoint else { return }
            link = result; qr = Self.makeQR(result)
        }
    }
    private func loadDevices() async {
        if let result = try? await store.client?.request("v1/security/devices"), case .array(let ids) = result["devices"] {
            deviceIDs = ids.compactMap(\.string).sorted()
            if result["pairingConsumed"].bool && link != nil { await generatePairing() }
        }
    }
    private func startTunnel() {
        Task { await store.perform {
            guard let control = UInt16(serverPort), control > 0,
                  let remote = UInt16(remotePort), remote > 0 else { throw HostFailure(message: "端口应为 1–65535") }
            guard let client = store.client else { throw HostFailure(message: "主机服务尚未连接") }
            store.error = nil
            do {
                _ = try await client.request("v1/tunnel", body: .object([
                    "server": .string(server), "serverPort": .number(Double(control)), "remotePort": .number(Double(remote)),
                    "token": .string(token), "trustedCaFile": caPath.isEmpty ? .null : .string(caPath)
                ]))
            } catch {
                await store.refresh()
                throw error
            }
            endpoint = Self.remoteURL(server: server, port: remote)
            token = ""
            await store.refresh()
        } }
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


struct TunnelStateLabel: View {
    let state: String?
    var body: some View {
        switch state {
        case "running": Label("运行中", systemImage: "network")
        case "exited": Label("连接已退出", systemImage: "exclamationmark.triangle")
        case "unknown": Label("状态未知", systemImage: "questionmark.circle")
        case "stopped": Label("未启动", systemImage: "network.slash")
        case nil: Label("未连接", systemImage: "network.slash")
        default: Label("状态未知", systemImage: "questionmark.circle")
        }
    }
}

private struct TunnelDiagnostics: View {
    let log: String
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if log.isEmpty {
                Text("暂无日志").foregroundStyle(.secondary)
            } else {
                Text(verbatim: log).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
            }
        }
    }
}

#Preview("连接设备") { ConnectionSettings(store: HostStore()) }
