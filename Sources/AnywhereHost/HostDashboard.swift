import AppKit
import SwiftUI

struct HostDashboard: View {
    @Bindable var store: HostStore
    @State private var showConnection = false
    @State private var showDesktop = false

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 24) {
                    HostDashboardHeader(available: store.serviceAvailable, starting: store.isStarting)
                    Button("连接设备", systemImage: "qrcode") { showConnection = true }
                        .controlSize(.large).modifier(HostPrimaryAction())
                        .disabled(!store.serviceAvailable)
                    HostServiceStatus(store: store)
                    HostServiceControls(store: store)
                }
                .padding(24)
                .frame(maxWidth: 580)
                .frame(maxWidth: .infinity)
            }
            .navigationTitle("Anywhere Host")
            .toolbar {
                ToolbarItemGroup(placement: .primaryAction) {
                    Button("权限", systemImage: store.permissions.toolbarSymbol) { store.permissions.openPermissions() }
                        .help("权限")
                    Button("远程桌面", systemImage: "display") { showDesktop = true }
                        .help(store.capabilities["remoteDesktop"]["supported"].bool ? "远程桌面" : "远程桌面不可用")
                        .disabled(!store.capabilities["remoteDesktop"]["supported"].bool)
                    Menu("更多", systemImage: "ellipsis") {
                        Button("打开日志", systemImage: "doc.text") { NSWorkspace.shared.open(store.process.stateDirectory.appending(path: "host.log")) }
                        Button("主机数据", systemImage: "externaldrive") { NSWorkspace.shared.open(store.process.stateDirectory) }
                        Button("设置向导", systemImage: "wand.and.stars") { store.permissions.openOnboarding() }
                    }
                }
            }
        }
        .sheet(isPresented: $showConnection) { ConnectionSettings(store: store) }
        .sheet(isPresented: $showDesktop, onDismiss: { Task { await store.refresh() } }) { HostDesktopSettings(store: store) }
        .alert("操作未完成", isPresented: Binding(get: { store.error != nil }, set: { if !$0 { store.error = nil } })) {
            Button("好", role: .cancel) { store.error = nil }
        } message: { Text(store.error ?? "") }
    }
}

private struct HostDashboardHeader: View {
    let available: Bool
    let starting: Bool
    var body: some View {
        VStack(spacing: 14) {
            Image(systemName: "desktopcomputer")
                .font(.system(size: 52, weight: .light)).foregroundStyle(.tint).accessibilityHidden(true)
            Text(Host.current().localizedName ?? "这台 Mac").font(.title.bold())
            Text("版本 \(HostVersion.display)").font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
            if starting { ProgressView("正在启动…").controlSize(.small) }
            else { HostStatusLabel(connected: available) }
        }.padding(.top, 8)
    }
}

private struct HostServiceStatus: View {
    let store: HostStore
    var body: some View {
        Grid(alignment: .leading, horizontalSpacing: 24, verticalSpacing: 18) {
            GridRow {
                Label("主机服务", systemImage: "server.rack")
                Text(store.isStarting ? "启动中" : store.serviceAvailable ? "运行中" : store.client == nil ? "已停止" : "连接中断")
                    .frame(maxWidth: .infinity, alignment: .trailing)
            }
            Divider().gridCellColumns(2)
            GridRow {
                Label("Codex", systemImage: "cpu")
                Text(store.runtimeLabel).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .trailing)
            }
            Divider().gridCellColumns(2)
            GridRow {
                Label("隧道", systemImage: "network")
                TunnelStateLabel(state: store.tunnel["state"].string).labelStyle(.titleOnly)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .trailing)
            }
            Divider().gridCellColumns(2)
            GridRow {
                Label("远程桌面", systemImage: "display")
                Text(desktopStatus).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .trailing)
            }
        }
        .padding(22)
        .background(.quaternary.opacity(0.35), in: .rect(cornerRadius: 20))
    }
    private var desktopStatus: LocalizedStringKey {
        guard store.serviceAvailable else { return "未连接" }
        guard store.capabilities["remoteDesktop"]["supported"].bool else { return "不支持" }
        guard store.desktop["enabled"].bool else { return "已关闭" }
        return store.desktop["reachable"].bool ? "可连接" : "不可连接"
    }
}

private struct HostServiceControls: View {
    let store: HostStore
    var body: some View {
        HStack(spacing: 16) {
            if store.client == nil {
                Button("启动主机", systemImage: "power") { Task { await store.start() } }
            } else {
                if !store.connected {
                    Button("重新连接", systemImage: "arrow.clockwise") { Task { await store.reconnectRuntime() } }
                }
                Button("停止主机", systemImage: "stop.circle") { store.stop() }
            }
        }.disabled(store.busy || store.isStarting)
    }
}

#Preview("主机") { HostDashboard(store: HostStore()).frame(width: 680, height: 640) }
#Preview("主机 · 深色") { HostDashboard(store: HostStore()).frame(width: 600, height: 600).preferredColorScheme(.dark) }
