import SwiftUI

/// Keeps setup presentation available independently of the currently selected thread.
struct HostSetupRootView: View {
    var store: HostStore
    var body: some View {
        HostWorkspace(store: store)
            .sheet(isPresented: Binding(
                get: { store.permissions.showOnboarding || store.permissions.showPermissions },
                set: { if !$0 && !store.permissions.showOnboarding { store.permissions.showPermissions = false } }
            )) {
                if store.permissions.showOnboarding { HostOnboardingView(store: store) }
                else { HostPermissionPage(store: store) }
            }
            .task {
                // Existing installations keep serving paired devices while the new guide is shown.
                let configured = FileManager.default.fileExists(atPath: store.process.stateDirectory.appending(path: "config.json").path)
                if !store.permissions.showOnboarding || configured { await prepareHost() }
            }
            .onChange(of: store.permissions.showOnboarding) { _, showing in
                if !showing { Task { await prepareHost() } }
            }
    }
    private func prepareHost() async {
        await store.start()
        await store.refresh()
        let center = store.permissions
        center.configure(stateDirectory: store.process.stateDirectory, codexHome: store.status["runtime"]["codexHome"].string, projectRoots: store.projects.flatMap(\.roots))
        if center.hasCompletedOnboarding && !center.showOnboarding && !center.showPermissions {
            await center.checkAll(includeOptional: false)
        }
    }
}

struct HostOnboardingView: View {
    var store: HostStore
    @State private var step = 0
    @State private var confirmSkip = false
    private var canFinish: Bool { store.permissions.canFinishChecks && store.connected }
    var body: some View {
        VStack(spacing: 0) {
            OnboardingProgress(step: step)
            Divider()
            ScrollView {
                if step == 0 { OnboardingWelcome().padding(32) }
                else if step == 1 { HostPermissionChecklist(store: store).padding(24) }
                else { OnboardingReady(store: store).padding(32) }
            }
            Divider()
            HStack {
                Button("稍后设置") { confirmSkip = true }.disabled(store.permissions.isChecking)
                Spacer()
                if step > 0 { Button("上一步") { step -= 1 }.disabled(store.permissions.isChecking) }
                if step == 0 {
                    Button("开始设置") { step = 1 }.buttonStyle(.borderedProminent)
                } else if step == 1 {
                    VStack(alignment: .trailing, spacing: 4) {
                        if !canFinish { Text("请完成必需目录检查并连接 Codex").font(.caption).foregroundStyle(.secondary) }
                        Button("继续") { step = 2 }.buttonStyle(.borderedProminent).disabled(!canFinish)
                    }
                } else {
                    Button("进入主机控制台") { store.permissions.completeOnboarding() }.buttonStyle(.borderedProminent).disabled(!canFinish)
                }
            }.padding(20)
        }
        .frame(width: 780, height: 720)
        .interactiveDismissDisabled()
        .task(id: step) { if step == 1 { await store.start(); await store.refresh() } }
        .onChange(of: canFinish) { _, ready in if !ready && step == 2 { step = 1 } }
        .confirmationDialog("稍后继续设置？", isPresented: $confirmSkip, titleVisibility: .visible) {
            Button("稍后设置") { store.permissions.deferOnboarding() }
            Button("继续设置", role: .cancel) { }
        } message: { Text("未完成的权限可能影响文件访问或工具执行。可随时打开权限检查；下次启动会继续提示设置。") }
    }
}

private struct OnboardingProgress: View {
    let step: Int
    var body: some View {
        HStack(spacing: 24) {
            Label("欢迎", systemImage: step > 0 ? "checkmark.circle.fill" : "1.circle.fill").foregroundStyle(step == 0 ? Color.accentColor : Color.secondary)
            Image(systemName: "chevron.right").foregroundStyle(.tertiary)
            Label("权限检查", systemImage: step > 1 ? "checkmark.circle.fill" : "2.circle.fill").foregroundStyle(step == 1 ? Color.accentColor : Color.secondary)
            Image(systemName: "chevron.right").foregroundStyle(.tertiary)
            Label("准备完成", systemImage: "3.circle.fill").foregroundStyle(step == 2 ? Color.accentColor : Color.secondary)
        }.font(.headline).padding(24).accessibilityElement(children: .combine)
    }
}

private struct OnboardingWelcome: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 28) {
            Image(systemName: "laptopcomputer.and.iphone").font(.system(size: 64)).foregroundStyle(.tint).accessibilityHidden(true)
            Text("让这台 Mac，成为你的随身工作台。").font(.largeTitle.bold())
            Text("花几分钟完成设置，让 iPhone 稳定地连接项目、继续会话和运行工具。")
                .font(.title3).foregroundStyle(.secondary)
            OnboardingFeature(symbol: "folder", title: "检查文件访问", detail: "验证主机数据、Codex Home 和实际项目目录，提前发现读写问题。")
            OnboardingFeature(symbol: "lock.shield", title: "逐项完成授权", detail: "按用途打开系统设置，授权后返回自动复查，提示何时需要重新打开应用。")
            OnboardingFeature(symbol: "qrcode", title: "连接你的 iPhone", detail: "确认 Codex 就绪后，设置远程连接并导入连接二维码。")
        }.frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct OnboardingFeature: View {
    let symbol: String
    let title: LocalizedStringResource
    let detail: LocalizedStringResource
    var body: some View {
        HStack(alignment: .top, spacing: 16) {
            Image(systemName: symbol).font(.title2).foregroundStyle(.tint).frame(width: 32).accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 6) { Text(title).font(.headline); Text(detail).foregroundStyle(.secondary) }
        }
    }
}

private struct OnboardingReady: View {
    var store: HostStore
    var body: some View {
        VStack(alignment: .leading, spacing: 24) {
            Image(systemName: "checkmark.circle.fill").font(.system(size: 64)).foregroundStyle(.green).accessibilityHidden(true)
            Text("基础检查已完成").font(.largeTitle.bold())
            Text("必需目录可读写，Codex 已连接。进入控制台后点击“连接 iPhone”完成远程连接设置。")
                .font(.title3).foregroundStyle(.secondary)
            LabeledContent("目录检查", value: "\(store.permissions.passedCount)/\(store.permissions.directories.count) 个可读写")
            LabeledContent("Codex", value: store.runtimeLabel)
            Text("按需使用的系统权限以实际工具的请求结果为准。更换项目、更新应用或遇到权限错误后，可随时重新检查。")
                .foregroundStyle(.secondary)
        }.frame(maxWidth: .infinity, alignment: .leading)
    }
}
