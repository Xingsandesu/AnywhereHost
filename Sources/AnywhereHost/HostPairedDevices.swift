import SwiftUI

struct PairedDevice: Identifiable {
    let id: String
    let connections: [DeviceLink]
    static func decode(_ value: JSONValue) -> [Self] {
        guard case .array(let ids) = value["devices"] else { return [] }
        return ids.compactMap { item in
            guard let id = item.string else { return nil }
            let connections: [JSONValue]
            if case .array(let items) = value["connections"][id] { connections = items } else { connections = [] }
            return Self(id: id, connections: connections.compactMap(DeviceLink.init))
        }
    }
}

struct DeviceLink: Identifiable {
    let id: String
    let value: JSONValue
    init?(_ value: JSONValue) {
        guard let id = value["connectionId"].string else { return nil }
        self.id = id; self.value = value
    }
}

struct HostPairedDevices: View {
    let store: HostStore
    @State private var revoking: PairedDevice?
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("已配对客户端").font(.headline)
            if store.devices.isEmpty {
                Text(store.serviceAvailable ? "尚未配对客户端" : "启动服务后查看客户端")
                    .foregroundStyle(.secondary)
            }
            ForEach(store.devices) { device in
                VStack(alignment: .leading, spacing: 8) {
                    HStack {
                        Label("设备 \(device.id.prefix(6))", systemImage: "iphone")
                            .font(.subheadline)
                        Spacer()
                        Button("解除配对", role: .destructive) { revoking = device }
                            .disabled(store.busy || !store.serviceAvailable)
                    }
                    if device.connections.isEmpty { Text("离线").font(.caption).foregroundStyle(.secondary) }
                    ForEach(device.connections) { link in
                        let connection = link.value
                        HStack {
                            Label(mode(connection), systemImage: connection["mode"].string == "direct" ? "point.3.connected.trianglepath.dotted" : "network")
                            Spacer()
                            if case .number(let ms) = connection["rttMs"] {
                                Text("延迟 \(ms, specifier: "%.0f") 毫秒")
                            } else { Text("延迟待测量") }
                        }.font(.caption).foregroundStyle(.secondary)
                    }
                }
            }
        }
        .padding(20).frame(maxWidth: .infinity, alignment: .leading)
        .background(.quaternary.opacity(0.35), in: .rect(cornerRadius: 20))
        .confirmationDialog("解除客户端配对？", isPresented: Binding(get: { revoking != nil }, set: { if !$0 { revoking = nil } }), titleVisibility: .visible) {
            if let device = revoking {
                Button("解除配对", role: .destructive) { revoking = nil; Task { await store.revoke(device.id) } }
            }
            Button("取消", role: .cancel) { revoking = nil }
        } message: { Text("将断开此客户端的连接并撤销授权。再次连接需要重新扫码配对。") }
    }
    private func mode(_ connection: JSONValue) -> LocalizedStringKey {
        switch connection["mode"].string {
        case "direct": "在线 · 设备直连"
        case "relay": "在线 · 加密转发"
        default: "在线 · 正在确认连接方式"
        }
    }
}
