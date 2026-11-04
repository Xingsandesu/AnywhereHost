import SwiftUI

/// Presents first-launch setup and host permissions.
struct HostSetupRootView: View {
    var store: HostStore
    var body: some View {
        HostDashboard(store: store)
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
        center.configure(stateDirectory: store.process.stateDirectory, codexHome: store.status["runtime"]["codexHome"].string)
        if center.hasCompletedOnboarding && !center.showOnboarding && !center.showPermissions {
            await center.checkAll(includeOptional: false)
        }
    }
}

struct HostOnboardingView: View {
    var store: HostStore
    @State private var step = 0
    private var canFinish: Bool { store.permissions.canFinishChecks && store.connected }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text(step == 0 ? "欢迎" : step == 1 ? "权限" : "就绪").font(.headline)
                Spacer()
                Text("\(step + 1) / 3").font(.callout.monospacedDigit()).foregroundStyle(.secondary)
            }.padding(24)
            if step == 1 {
                HostPermissionChecklist(store: store)
            } else {
                Spacer()
                VStack(spacing: 24) {
                    Image(systemName: step == 0 ? "desktopcomputer" : "checkmark.circle.fill")
                        .font(.system(size: 72, weight: .light))
                        .foregroundStyle(step == 0 ? Color.accentColor : Color.green)
                        .accessibilityHidden(true)
                    Text(step == 0 ? "Anywhere Host" : "准备就绪").font(.largeTitle.bold())
                    if step == 2 { HostStatusLabel(connected: store.connected) }
                }
                Spacer()
            }
            HStack {
                Button("稍后设置") { store.permissions.deferOnboarding() }.disabled(store.permissions.isChecking)
                Spacer()
                if step > 0 {
                    Button("上一步") { step -= 1 }.disabled(store.permissions.isChecking)
                }
                Button(step == 0 ? "开始设置" : step == 1 ? "继续" : "完成") {
                    if step < 2 { step += 1 } else { store.permissions.completeOnboarding() }
                }
                .modifier(HostPrimaryAction()).keyboardShortcut(.defaultAction)
                .disabled(step > 0 && !canFinish)
            }.controlSize(.large).padding(24)
        }
        .frame(width: 680, height: 620)
        .interactiveDismissDisabled()
        .task(id: step) { if step == 1 { await store.start(); await store.refresh() } }
        .onChange(of: canFinish) { _, ready in if !ready && step == 2 { step = 1 } }
    }
}

#Preview("设置") { HostOnboardingView(store: HostStore()) }
