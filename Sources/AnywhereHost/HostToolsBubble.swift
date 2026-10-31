import SwiftUI

struct HostToolsBubble: View {
    let store: HostStore
    let onDetails: () -> Void
    @State private var tab = 0
    var body: some View {
        VStack(spacing: 14) {
            Picker("工具", selection: $tab) {
                Label("设置", systemImage: "slider.horizontal.3").labelStyle(.iconOnly).tag(0)
                Label("原生操作", systemImage: "bolt").labelStyle(.iconOnly).tag(1)
            }.pickerStyle(.segmented)
            if tab == 0 {
                Picker("模型", selection: Binding(get: { store.settings.model }, set: { value in update { $0.model = value; $0.effort = "" } })) {
                    Text("主机默认").tag("")
                    ForEach(store.models, id: \.modelID) { Text($0["displayName"].string ?? $0.modelID).tag($0.modelID) }
                }
                Picker("推理", selection: Binding(get: { store.settings.effort }, set: { value in update { $0.effort = value } })) {
                    Text("默认").tag("")
                    ForEach(store.models.first(where: { $0.modelID == store.settings.model })?["supportedReasoningEfforts"].array.compactMap { $0["reasoningEffort"].string } ?? [], id: \.self) { Text($0).tag($0) }
                }
                Picker("权限", selection: Binding(get: { store.settings.access }, set: { value in update { $0.selectAccess(value) } })) {
                    ForEach(CodexAccess.allCases) { Text($0.title).tag($0) }
                }
                Toggle("计划模式", isOn: Binding(get: { store.settings.plan == true }, set: { value in update { $0.plan = value } }))
                if store.settings.access == .dangerFullAccess { Text("全部文件与网络访问，不再请求批准。").font(.caption).foregroundStyle(.secondary) }
                Button(action: onDetails) { Image(systemName: "gearshape").frame(maxWidth: .infinity) }.help("详细设置")
            } else {
                HStack(spacing: 16) {
                    icon("分叉", "arrow.triangle.branch", enabled: store.supports("thread/fork")) { try await store.forkThread() }
                    icon("压缩", "arrow.down.right.and.arrow.up.left", enabled: allowed("thread/compact/start")) { try await store.compact() }
                    icon("审查", "checkmark.shield", enabled: allowed("review/start")) { try await store.nativeAction("review/start") }
                    icon("加入队列", "text.badge.plus", enabled: store.canMutate && !store.draft.isEmpty && store.supports("thread/queue/add")) { try await store.nativeAction("thread/queue/add") }
                    icon("追加指令", "arrow.turn.down.right", enabled: store.canMutate && store.activeTurn != nil && !store.draft.isEmpty && store.supports("turn/steer")) { try await store.nativeAction("turn/steer") }
                }
            }
        }.padding(16).frame(width: 310).disabled(store.busy || !store.connected)
        .task { await store.perform { try await store.loadSettingsCatalog() } }
    }
    private func allowed(_ method: String) -> Bool { store.canMutate && store.activeTurn == nil && store.supports(method) }
    private func update(_ edit: (inout HostCodexSettings) -> Void) {
        var value = store.settings; edit(&value)
        Task { await store.perform { try await store.applySettings(value) } }
    }
    private func icon(_ name: String, _ symbol: String, enabled: Bool, _ action: @escaping @MainActor () async throws -> Void) -> some View {
        Button { Task { await store.perform(action) } } label: { Image(systemName: symbol).frame(width: 44, height: 44) }
            .help(name).accessibilityLabel(name).disabled(!enabled)
    }
}

struct HostSettingsSheet: View {
    let store: HostStore
    @State private var value = HostCodexSettings()
    @State private var defaults = false
    @Environment(\.dismiss) private var dismiss
    var body: some View {
        VStack(spacing: 16) {
            Form {
                Toggle("新会话默认设置", isOn: $defaults)
                    .onChange(of: defaults) { _, yes in value = yes ? store.defaultSettings : store.settings }
                Picker("权限", selection: Binding(get: { value.access }, set: { value.selectAccess($0) })) {
                    ForEach(CodexAccess.allCases) { Text($0.title).tag($0) }
                }
                Picker("命名权限配置", selection: $value.permissionProfile) {
                    Text("使用上方权限").tag("")
                    ForEach(store.permissionProfiles, id: \.profileID) { profile in
                        Text(profile["description"].string ?? profile.profileID).tag(profile.profileID).disabled(!profile["allowed"].bool)
                    }
                }
                Picker("执行批准", selection: $value.approval) {
                    Text("主机默认").tag("")
                    Text("按需询问").tag("on-request")
                    Text("不受信任的命令需批准").tag("untrusted")
                    Text("从不询问").tag("never")
                }.disabled(value.access == .dangerFullAccess)
                Toggle("允许网络访问", isOn: Binding(get: { value.network ?? false }, set: { value.network = $0 }))
                    .disabled(value.access == .dangerFullAccess || value.access == .inherit || !value.permissionProfile.isEmpty)
                TextField("模型", text: $value.model)
                TextField("推理强度", text: $value.effort)
                Toggle("计划模式", isOn: Binding(get: { value.plan == true }, set: { value.plan = $0 }))
            }
            HStack {
                Button("取消") { dismiss() }
                Spacer()
                Button("应用") {
                    Task { await store.perform {
                        _ = try value.parameters(currentModel: store.context["model"].string)
                        if defaults { store.defaultSettings = value } else { try await store.applySettings(value) }
                        dismiss()
                    } }
                }.disabled(store.busy || (!defaults && (!store.canMutate || !store.connected)))
            }
        }.padding(24).frame(width: 460).onAppear { value = store.settings }
    }
}

extension JSONValue {
    var modelID: String { self["model"].string ?? self["id"].string ?? "" }
    var profileID: String { self["id"].string ?? "" }
}
