import AppKit
@preconcurrency import ApplicationServices
import SwiftUI

struct HostPermissionsView: View {
    var store: HostStore
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Label("权限与运行检查", systemImage: store.permissions.toolbarSymbol).font(.headline)
            Text("检查项目读写、系统授权和 Codex 连接，提前处理远程使用时的访问问题。")
                .foregroundStyle(.secondary)
            if store.permissions.lastChecked != nil {
                Text("\(store.permissions.passedCount)/\(store.permissions.directories.count) 个目录可读写").font(.caption)
            }
            HStack {
                Button("打开权限检查") { store.permissions.openPermissions() }
                Button("重新查看使用引导") { store.permissions.openOnboarding() }
            }
            HStack {
                if store.client == nil { Button("启动主机") { Task { await store.start() } } }
                else { Button("停止主机") { store.stop() } }
                Button("打开主机数据目录") { NSWorkspace.shared.open(store.process.stateDirectory) }
            }.disabled(store.busy || store.isStarting)
        }
        .font(.callout).padding(20).frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(nsColor: .controlBackgroundColor), in: .rect(cornerRadius: 14))
    }
}

struct HostPermissionPage: View {
    var store: HostStore
    @Environment(\.dismiss) private var dismiss
    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("权限与运行检查").font(.title2.bold())
                Spacer()
                Button("完成") { dismiss() }.keyboardShortcut(.cancelAction)
            }.padding(24)
            Divider()
            ScrollView { HostPermissionChecklist(store: store).padding(24) }
        }.frame(width: 760, height: 720)
    }
}

struct HostPermissionChecklist: View {
    var store: HostStore
    @State private var checkTask: Task<Void, Never>?
    @State private var accessibilityGranted = false
    @State private var screenGranted = false
    private var center: HostPermissionCenter { store.permissions }
    var body: some View {
        VStack(alignment: .leading, spacing: 24) {
            PermissionCheckHeader(center: center, check: runChecks, choose: {
                checkTask = Task { if await center.chooseDirectory() { configure(); await center.checkAll() } }
            }, cancel: { center.cancelChecks(); checkTask?.cancel() })
            PermissionSystemGuidance(center: center, shared: store.status["runtime"]["sharesLiveProcess"].bool)
            VStack(alignment: .leading, spacing: 12) {
                Text("文件访问").font(.title3.bold())
                Text("主机数据、Codex Home 和项目目录需要可读写。文稿、桌面和下载目录按需授权。")
                    .font(.callout).foregroundStyle(.secondary)
                ForEach(center.directories) { directory in
                    PermissionDirectoryRow(directory: directory, state: center.states[directory.id] ?? .unchecked)
                }
            }
            PermissionRuntimeChecks(store: store)
            PermissionOptionalAccess(center: center, accessibility: accessibilityGranted, screen: screenGranted, requestAccessibility: {
                _ = AXIsProcessTrustedWithOptions([kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary)
                refreshSystemStatus()
                if !accessibilityGranted { center.openSettings(.accessibility) }
            }, requestScreen: {
                _ = CGRequestScreenCaptureAccess()
                refreshSystemStatus()
                if !screenGranted { center.openSettings(.screenRecording) }
            })
            if let error = center.settingsError { Label(error, systemImage: "exclamationmark.triangle").foregroundStyle(.orange) }
        }
        .task { configure(); refreshSystemStatus() }
        .onChange(of: store.projects.map(\.roots)) { _, _ in configure() }
        .onChange(of: store.status["runtime"]["codexHome"].string) { _, _ in configure() }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            refreshSystemStatus()
            if center.hasRequestedCheck { runChecks() }
        }
        .onDisappear { checkTask?.cancel() }
    }
    private func configure() {
        center.configure(stateDirectory: store.process.stateDirectory, codexHome: store.status["runtime"]["codexHome"].string, projectRoots: store.projects.flatMap(\.roots))
    }
    private func refreshSystemStatus() { accessibilityGranted = AXIsProcessTrusted(); screenGranted = CGPreflightScreenCaptureAccess() }
    private func runChecks() {
        guard !center.isChecking else { return }
        configure(); refreshSystemStatus()
        checkTask = Task { await center.checkAll() }
    }
}

private struct PermissionCheckHeader: View {
    var center: HostPermissionCenter
    let check: () -> Void
    let choose: () -> Void
    let cancel: () -> Void
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("让每次远程工作都顺利开始").font(.title2.bold())
            Text("先检查真实的文件访问，再按需开启系统权限。授权完成后返回这里自动复查。")
                .foregroundStyle(.secondary)
            HStack {
                Button(center.hasRequestedCheck ? "重新检查" : "开始检查", systemImage: "arrow.clockwise", action: check).buttonStyle(.borderedProminent)
                Button("选择项目目录…", systemImage: "folder", action: choose)
                Spacer()
                if center.isChecking { ProgressView().controlSize(.small) }
                else if center.lastChecked != nil { Text("\(center.passedCount)/\(center.directories.count) 个可读写").font(.callout.monospacedDigit()) }
            }.disabled(center.isChecking)
            if center.isChecking { Button("停止检查", action: cancel) }
        }
    }
}

private struct PermissionSystemGuidance: View {
    var center: HostPermissionCenter
    let shared: Bool
    var body: some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 10) {
                Text("1. 在系统设置中找到 Anywhere Host，开启所需访问权限。完全磁盘访问列表里没有应用时，用“＋”添加当前应用。")
                Text("2. 授权后如仍未通过，请完全退出并重新打开 Anywhere Host，再运行检查。")
                if shared {
                    Text("3. 当前任务在共享 Codex 进程中执行。请同时为 Codex 和实际调用工具的应用授权，并按需重新打开；Host 的结果不代表其他进程已获准。")
                } else {
                    Text("3. Xcode、终端和 MCP 工具可能有各自的提示，请在实际执行操作的应用中授权。此页不改变 Codex 的命令审批设置。")
                }
                HStack {
                    Button("文件与文件夹") { center.openSettings(.files) }
                    Button("完全磁盘访问权限") { center.openSettings(.fullDisk) }
                    Button("显示当前应用") { NSWorkspace.shared.activateFileViewerSelecting([Bundle.main.bundleURL]) }
                }
                Text("完全磁盘访问：需在设置中手动确认。下方仅验证列出的目录，不将局部读写成功当作全部磁盘已授权。")
                    .foregroundStyle(.secondary)
            }.font(.callout).fixedSize(horizontal: false, vertical: true).frame(maxWidth: .infinity, alignment: .leading).padding(6)
        } label: { Label("授权与生效", systemImage: "lock.shield") }
    }
}

private struct PermissionDirectoryRow: View {
    let directory: PermissionDirectory
    let state: DirectoryPermissionState
    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: state == .ready ? "checkmark.circle.fill" : "folder.badge.questionmark")
                .foregroundStyle(state == .ready ? Color.green : Color.secondary).font(.title3).accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 4) {
                HStack {
                    Text(verbatim: directory.title).fontWeight(.medium)
                    Text(directory.required ? "必需" : "按需").font(.caption).foregroundStyle(.secondary)
                    Spacer()
                    Text(state.title).font(.callout).foregroundStyle(state == .ready ? Color.green : Color.secondary)
                }
                Text(verbatim: directory.path).font(.caption.monospaced()).textSelection(.enabled)
                Text(state.detail).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
        }.padding(14).background(.quaternary.opacity(0.4), in: .rect(cornerRadius: 10)).accessibilityElement(children: .combine)
    }
}

private struct PermissionRuntimeChecks: View {
    var store: HostStore
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("运行与连接").font(.title3.bold())
            LabeledContent("本机 Host 服务", value: store.client == nil ? "未连接" : "可连接")
            LabeledContent("Codex", value: store.connected ? store.runtimeLabel : "未连接")
            if !store.connected {
                Text(store.status["error"].string ?? "请打开并登录 Codex，或确认已安装可用的 Codex CLI。")
                    .font(.callout).foregroundStyle(.secondary).textSelection(.enabled)
                Button(store.client == nil ? "启动 Host" : "重新连接 Codex") {
                    Task {
                        if store.client == nil { await store.start() }
                        else { await store.perform {
                            _ = try await store.client?.request("v1/runtime/connect", body: .object([:]))
                            await store.refresh()
                        } }
                    }
                }.disabled(store.isStarting || store.busy)
            }
            if let error = store.error { Text(error).font(.callout).foregroundStyle(.orange).textSelection(.enabled) }
            Text("本机连接成功不代表 iPhone 已能访问。完成授权后，在“连接 iPhone”中设置隧道并用手机验证。")
                .font(.callout).foregroundStyle(.secondary)
        }
    }
}

private struct PermissionOptionalAccess: View {
    var center: HostPermissionCenter
    let accessibility: Bool
    let screen: Bool
    let requestAccessibility: () -> Void
    let requestScreen: () -> Void
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("工具使用的系统权限").font(.title3.bold())
            Text("按任务需要开启，不影响基础项目和会话功能。自动检测仅针对当前 Host 应用。")
                .font(.callout).foregroundStyle(.secondary)
            GroupBox {
                VStack(alignment: .leading, spacing: 14) {
                    PermissionCapabilityRow(title: "辅助功能", detail: "操作其他应用的界面时使用。", status: accessibility ? "已授权" : "未授权", button: "请求授权", granted: accessibility, action: requestAccessibility)
                    Divider()
                    PermissionCapabilityRow(title: "屏幕录制", detail: "截屏或查看屏幕时使用，授权后可能需要重新打开应用。", status: screen ? "已授权" : "未授权", button: "请求授权", granted: screen, action: requestScreen)
                    Divider()
                    PermissionCapabilityRow(title: "自动化", detail: "按目标应用分别授权。首次运行对应工具时处理系统提示，再到设置中确认。", status: "需手动确认", button: "打开设置", granted: false) { center.openSettings(.automation) }
                    Divider()
                    PermissionCapabilityRow(title: "本地网络", detail: "访问局域网服务器或设备时可能需要。本机回环连接不能验证此权限。", status: "按需确认", button: "打开设置", granted: false) { center.openSettings(.localNetwork) }
                }.padding(6)
            }
        }
    }
}

private struct PermissionCapabilityRow: View {
    let title: LocalizedStringResource
    let detail: LocalizedStringResource
    let status: LocalizedStringResource
    let button: LocalizedStringResource
    let granted: Bool
    let action: () -> Void
    var body: some View {
        HStack(alignment: .top, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                HStack { Text(title).fontWeight(.medium); Text(status).font(.caption).foregroundStyle(granted ? Color.green : Color.secondary) }
                Text(detail).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 0)
            if !granted { Button(button, action: action) }
        }
    }
}
