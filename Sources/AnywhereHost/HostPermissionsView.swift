import AppKit
@preconcurrency import ApplicationServices
import SwiftUI

struct HostPermissionPage: View {
    let store: HostStore
    var body: some View {
        HostSheet(title: "权限") {
            HostPermissionChecklist(store: store)
        }.frame(width: 680, height: 620)
    }
}

struct HostPermissionChecklist: View {
    let store: HostStore
    @State private var checkTask: Task<Void, Never>?
    @State private var accessibilityGranted = false
    @State private var screenGranted = false
    private var center: HostPermissionCenter { store.permissions }

    var body: some View {
        Form {
            Section("文件访问") {
                HStack {
                    Button(center.hasRequestedCheck ? "重新检查" : "开始检查", systemImage: "arrow.clockwise", action: runChecks)
                        .disabled(center.isChecking)
                    Button("添加目录", systemImage: "folder.badge.plus") {
                        checkTask = Task { if await center.chooseDirectory() { configure(); await center.checkAll() } }
                    }.disabled(center.isChecking)
                    Spacer()
                    if center.isChecking {
                        ProgressView().controlSize(.small)
                        Button("停止") { center.cancelChecks(); checkTask?.cancel() }
                    } else if center.lastChecked != nil {
                        Text("\(center.passedCount) / \(center.directories.count)").monospacedDigit().foregroundStyle(.secondary)
                    }
                }
                ForEach(center.directories) { directory in
                    PermissionDirectoryRow(directory: directory, state: center.states[directory.id] ?? .unchecked)
                }
            }
            Section("系统权限") {
                LabeledContent("文件与文件夹") { Button("系统设置") { center.openSettings(.files) } }
                LabeledContent("完全磁盘访问") { Button("系统设置") { center.openSettings(.fullDisk) } }
                PermissionCapabilityRow(title: "辅助功能", granted: accessibilityGranted) {
                    _ = AXIsProcessTrustedWithOptions([kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary)
                    refreshSystemStatus()
                    if !accessibilityGranted { center.openSettings(.accessibility) }
                }
                PermissionCapabilityRow(title: "屏幕录制", granted: screenGranted) {
                    _ = CGRequestScreenCaptureAccess()
                    refreshSystemStatus()
                    if !screenGranted { center.openSettings(.screenRecording) }
                }
                LabeledContent("自动化") { Button("系统设置") { center.openSettings(.automation) } }
                LabeledContent("本地网络") { Button("系统设置") { center.openSettings(.localNetwork) } }
                Button("在 Finder 中显示应用", systemImage: "arrow.up.forward.app") {
                    NSWorkspace.shared.activateFileViewerSelecting([Bundle.main.bundleURL])
                }
            }
            Section("连接") { PermissionRuntimeChecks(store: store) }
            if let error = center.settingsError {
                Label(error, systemImage: "exclamationmark.triangle").foregroundStyle(.orange)
            }
        }
        .formStyle(.grouped)
        .task { configure(); refreshSystemStatus() }
        .onChange(of: store.status["runtime"]["codexHome"].string) { _, _ in configure() }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            refreshSystemStatus()
            if center.hasRequestedCheck { runChecks() }
        }
        .onDisappear { center.cancelChecks(); checkTask?.cancel() }
    }

    private func configure() {
        center.configure(stateDirectory: store.process.stateDirectory, codexHome: store.status["runtime"]["codexHome"].string)
    }
    private func refreshSystemStatus() { accessibilityGranted = AXIsProcessTrusted(); screenGranted = CGPreflightScreenCaptureAccess() }
    private func runChecks() {
        guard !center.isChecking else { return }
        configure(); refreshSystemStatus()
        checkTask = Task { await center.checkAll() }
    }
}

private struct PermissionDirectoryRow: View {
    let directory: PermissionDirectory
    let state: DirectoryPermissionState
    var body: some View {
        LabeledContent {
            Text(state.title).foregroundStyle(state == .ready ? Color.green : Color.secondary)
        } label: {
            VStack(alignment: .leading, spacing: 4) {
                Label(directory.title, systemImage: state == .ready ? "checkmark.circle" : "folder")
                Text(verbatim: directory.path).font(.caption).foregroundStyle(.secondary)
                    .lineLimit(1).truncationMode(.middle).textSelection(.enabled)
            }
        }.accessibilityElement(children: .combine)
    }
}

private struct PermissionRuntimeChecks: View {
    let store: HostStore
    var body: some View {
        LabeledContent("主机", value: store.client == nil ? "未连接" : "已连接")
        LabeledContent("Codex", value: store.runtimeLabel)
        if !store.connected {
            if let error = store.status["error"].string { Text(error).foregroundStyle(.secondary).textSelection(.enabled) }
            Button(store.client == nil ? "启动主机" : "重新连接") {
                Task {
                    if store.client == nil { await store.start() }
                    else { await store.perform {
                        _ = try await store.client?.request("runtime.connect", body: .object([:]))
                        await store.refresh()
                    } }
                }
            }.disabled(store.isStarting || store.busy)
        }
        if let error = store.error { Text(error).foregroundStyle(.orange).textSelection(.enabled) }
    }
}

private struct PermissionCapabilityRow: View {
    let title: LocalizedStringKey
    let granted: Bool
    let action: () -> Void
    var body: some View {
        LabeledContent(title) {
            if granted { Label("已授权", systemImage: "checkmark.circle").foregroundStyle(.green) }
            else { Button("授权", action: action) }
        }
    }
}

#Preview("权限") { HostPermissionPage(store: HostStore()) }
