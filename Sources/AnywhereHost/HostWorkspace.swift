import AppKit
import SwiftUI

struct HostWorkspace: View {
    @Bindable var store: HostStore
    @State private var showConnection = false
    @State private var showDesktop = false
    var body: some View {
        NavigationSplitView {
            ProjectSidebar(store: store)
                .navigationSplitViewColumnWidth(min: 200, ideal: 240)
        } content: {
            ThreadSidebar(store: store)
                .navigationSplitViewColumnWidth(min: 230, ideal: 280)
        } detail: {
            if store.selectedThread != nil { ConversationDetail(store: store).id(store.selectedThread) }
            else { HostOverview(store: store, showConnection: $showConnection) }
        }
        .toolbar {
            ToolbarItem { Label(store.connected ? "主机在线" : "主机离线", systemImage: store.connected ? "circle.fill" : "circle.dotted").foregroundStyle(store.connected ? .green : .secondary).font(.caption) }
            ToolbarItem { Button("权限检查", systemImage: store.permissions.toolbarSymbol) { store.permissions.openPermissions() } }
            ToolbarItem { Button("远程桌面", systemImage: "display") { showDesktop = true } }
            ToolbarItem { Button("连接 iPhone", systemImage: "qrcode") { showConnection = true } }
        }
        .sheet(isPresented: $showConnection) { ConnectionSettings(store: store) }
        .sheet(isPresented: $showDesktop) { HostDesktopSettings(store: store) }
        .alert("操作未完成", isPresented: Binding(get: { store.error != nil }, set: { if !$0 { store.error = nil } })) {
            Button("知道了", role: .cancel) { store.error = nil }
        } message: { Text(store.error ?? "") }
    }
}
struct ProjectSidebar: View {
    @Bindable var store: HostStore
    var body: some View {
        List(selection: $store.selectedProject) {
            Section("主机工作区") {
                Label("所有会话", systemImage: "tray.full").tag(String?.none)
                ForEach(store.projects) { project in
                    VStack(alignment: .leading, spacing: 4) {
                        Label(project.name, systemImage: "folder")
                        if let root = project.roots.first { Text(verbatim: root).font(.caption2).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle) }
                    }.padding(.vertical, 4).tag(Optional(project.id))
                }
            }
        }
        .navigationTitle("Anywhere")
        .safeAreaInset(edge: .bottom) {
            VStack(alignment: .leading, spacing: 12) {
                Button("添加项目", systemImage: "folder.badge.plus") { Task { await store.perform { try await store.createProject() } } }
                    .disabled(!store.connected || store.busy)
                Label(store.runtimeLabel, systemImage: "desktopcomputer").font(.caption).foregroundStyle(.secondary)
            }.frame(maxWidth: .infinity, alignment: .leading).padding()
        }
        .task(id: store.selectedProject) { store.selectedThread = nil; await store.loadThreads() }
        .onChange(of: store.connected) { _, ready in if ready { Task { await store.loadThreads() } } }
    }
}
struct ThreadSidebar: View {
    @Bindable var store: HostStore
    var body: some View {
        List(selection: $store.selectedThread) {
            ForEach(store.threads) { thread in
                VStack(alignment: .leading, spacing: 6) {
                    Text(verbatim: thread.title).lineLimit(2)
                    Text(verbatim: thread.status).font(.caption).foregroundStyle(.secondary)
                }.padding(.vertical, 5).tag(Optional(thread.id))
            }
            if store.nextThreadCursor != nil { Button("加载更多会话") { Task { await store.loadThreads(more: true) } } }
        }
        .overlay { if store.threads.isEmpty { ContentUnavailableView("选择项目或创建会话", systemImage: "bubble.left.and.bubble.right", description: Text("直接使用主机 Codex 的会话与工作目录")) } }
        .navigationTitle("会话")
        .toolbar {
            ToolbarItem { Button("新建会话", systemImage: "square.and.pencil") { Task { await store.perform { try await store.createThread() } } }.disabled(store.selectedProject == nil || store.busy || !store.connected) }
            ToolbarItem { Button("刷新", systemImage: "arrow.clockwise") { Task { await store.loadThreads() } } }
        }
        .task(id: store.selectedThread) { await store.loadHistory() }
    }
}
struct HostOverview: View {
    var store: HostStore
    @Binding var showConnection: Bool
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                Image(systemName: "laptopcomputer.and.iphone").font(.system(size: 52)).foregroundStyle(.tint)
                Text("你的 Mac，随身可达。", comment: "Host welcome heading").font(.largeTitle.bold())
                Text("为 iPhone Duo 提供项目、会话、上下文和执行状态。所有工作都在这台 Mac 的 Codex 环境中完成。")
                    .font(.title3).foregroundStyle(.secondary)
                HostPermissionsView(store: store)
                RuntimeStatusCard(store: store)
                HStack(spacing: 24) {
                    StatusMetric(title: "主机项目", value: String(store.projects.count), symbol: "folder")
                    StatusMetric(title: "待处理请求", value: String(store.pending.count), symbol: "hand.raised")
                    StatusMetric(title: "隧道", value: store.tunnel["state"].string ?? "stopped", symbol: "network")
                }
                Button("设置远程连接", systemImage: "qrcode") { showConnection = true }.buttonStyle(.borderedProminent).controlSize(.large)
            }.padding(36).frame(maxWidth: 800, alignment: .leading)
        }
    }
}
struct RuntimeStatusCard: View {
    var store: HostStore
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Label("Codex 连接", systemImage: "desktopcomputer").font(.headline)
            HStack { Text("运行方式").foregroundStyle(.secondary); Spacer(); Text(store.runtimeLabel) }
            HStack(alignment: .top) {
                Text("Codex Home").foregroundStyle(.secondary)
                Spacer()
                Text(verbatim: store.status["runtime"]["codexHome"].string ?? FileManager.default.homeDirectoryForCurrentUser.appending(path: ".codex").path).font(.system(.callout, design: .monospaced)).textSelection(.enabled)
            }
            Text(store.status["runtime"]["note"].string ?? store.status["error"].string ?? "正在连接主机 Codex…")
                .font(.callout).foregroundStyle(.secondary).textSelection(.enabled)
            if !store.connected {
                Button("重新连接 Codex") { Task { await store.perform { _ = try await store.client?.request("v1/runtime/connect", body: .object([:])); await store.refresh() } } }.disabled(store.busy || store.client == nil)
            }
        }
        .padding(20).frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(nsColor: .controlBackgroundColor), in: .rect(cornerRadius: 14))
    }
}
struct StatusMetric: View {
    let title: LocalizedStringKey; let value: String; let symbol: String
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label(title, systemImage: symbol).foregroundStyle(.secondary)
            Text(verbatim: value).font(.title2.monospacedDigit())
        }.frame(maxWidth: .infinity, alignment: .leading)
    }
}
struct ConversationDetail: View {
    @Bindable var store: HostStore
    @State private var showContext = false
    @State private var visibleCount = 100
    var body: some View {
        VStack(spacing: 0) {
            if !store.status["runtime"]["sharesLiveProcess"].bool {
                Text("桌面原有会话可读取。继续已有上下文时，请先使用“分叉会话”；接入共享进程后可直接续写。")
                    .font(.caption).foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: .leading).padding().background(.quaternary.opacity(0.4))
            }
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 20) {
                    if store.nextHistoryCursor != nil || store.items.count > visibleCount {
                        Button("加载更早内容") {
                            Task {
                                if store.items.count <= visibleCount { await store.loadHistory(more: true) }
                                visibleCount += 100
                            }
                        }.disabled(store.loadingHistory)
                    }
                    if store.loadingHistory && store.items.isEmpty { ProgressView("读取会话…") }
                    if let error = store.historyError {
                        Text(error).font(.caption).foregroundStyle(.secondary)
                        Button("重试") { Task { await store.loadHistory() } }
                    }
                    ForEach(store.items.suffix(visibleCount)) { item in TimelineRow(item: item) }
                }.frame(maxWidth: 900, alignment: .leading).padding(24)
            }
            if !store.pending.isEmpty { ApprovalPanel(store: store) }
            Divider()
            ComposerPanel(store: store)
        }
        .toolbar {
            ToolbarItem { Button("分叉会话", systemImage: "arrow.triangle.branch") { Task { await store.perform { try await store.forkThread() } } }.disabled(store.busy) }
            ToolbarItem { Button("压缩上下文", systemImage: "arrow.down.right.and.arrow.up.left") { Task { await store.perform { try await store.compact() } } }.disabled(store.busy || store.activeTurn != nil) }
            ToolbarItem { Button("上下文", systemImage: "info.circle") { showContext = true } }
        }
        .sheet(isPresented: $showContext) {
            VStack(alignment: .leading, spacing: 16) {
                Text("会话上下文").font(.title2)
                Text("续接或新建会话后展示 Codex 返回的模型、权限和工作目录。").foregroundStyle(.secondary)
                ScrollView { Text(verbatim: store.context.pretty).font(.system(.caption, design: .monospaced)).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading) }
                Button("完成") { showContext = false }
            }.padding(24).frame(width: 650, height: 480)
        }
    }
}
struct TimelineRow: View {
    let item: TimelineItem
    @GestureState private var holding = false
    @State private var accessibleExpanded = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    private var progress: Bool { !["userMessage", "agentMessage", "plan"].contains(item.kind) }
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if progress {
                HStack {
                    Image(systemName: item.kind == "commandExecution" ? "terminal" : "sparkle")
                    Text(String(item.text.prefix(240).split(separator: "\n", maxSplits: 1).first ?? "")).lineLimit(1)
                    Spacer(minLength: 0)
                    Image(systemName: "chevron.down").rotationEffect(.degrees(holding || accessibleExpanded ? 180 : 0))
                }
                .font(.callout).foregroundStyle(.secondary).frame(minHeight: 32).contentShape(.rect)
                .gesture(LongPressGesture(minimumDuration: 0.3).sequenced(before: DragGesture(minimumDistance: 0))
                    .updating($holding) { value, state, _ in if case .second(true, _) = value { state = true } })
                if holding || accessibleExpanded {
                    Text(verbatim: String(item.text.prefix(12_000))).font(.caption.monospaced()).lineLimit(20)
                        .transition(.opacity.combined(with: .move(edge: .top)))
                }
            } else {
                Label(item.kind == "userMessage" ? "你" : "Codex", systemImage: item.kind == "userMessage" ? "person.crop.circle" : "sparkle")
                    .font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                Text(verbatim: String(item.text.prefix(24_000))).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .padding(progress ? 10 : 16).frame(maxWidth: .infinity, alignment: .leading)
        .background(.quaternary.opacity(item.kind == "userMessage" ? 0.6 : 0.2), in: .rect(cornerRadius: 12))
        .clipped().animation(reduceMotion ? nil : .smooth(duration: 0.22), value: holding || accessibleExpanded)
        .accessibilityAction(named: accessibleExpanded ? "收起详情" : "展开详情") { accessibleExpanded.toggle() }
        .contextMenu { Button("复制完整内容") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(item.text, forType: .string) } }
    }
}
struct ComposerPanel: View {
    @Bindable var store: HostStore
    @State private var showsTools = false
    @State private var showsSettings = false
    @FocusState private var focused: Bool
    var body: some View {
        HStack(alignment: .bottom, spacing: 12) {
            Button {
                focused = false; showsTools.toggle()
            } label: { Image(systemName: "plus").frame(width: 32, height: 32) }
                .buttonStyle(.borderless).help("工具")
                .popover(isPresented: $showsTools) {
                    HostToolsBubble(store: store) { showsTools = false; showsSettings = true }
                }
            TextField("继续当前会话…", text: $store.draft, axis: .vertical)
                .lineLimit(1...7).textFieldStyle(.plain).focused($focused)
            Button {
                Task { await store.perform { if store.activeTurn != nil { try await store.interrupt() } else { try await store.send() } } }
            } label: { Image(systemName: store.activeTurn != nil ? "stop.fill" : "arrow.up").frame(width: 32, height: 32) }
                .buttonStyle(.borderedProminent).keyboardShortcut(.return, modifiers: .command)
                .help(store.activeTurn != nil ? "停止" : "发送")
                .disabled(store.busy || !store.connected || !store.canMutate || (store.activeTurn == nil && store.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty))
        }
        .padding(20)
        .onChange(of: store.selectedThread) { _, _ in focused = false; showsTools = false }
        .sheet(isPresented: $showsSettings) { HostSettingsSheet(store: store) }
    }
}
struct ApprovalPanel: View {
    var store: HostStore
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                ForEach(store.pending) { request in
                    VStack(alignment: .leading, spacing: 8) {
                        Label("Codex 请求确认", systemImage: "hand.raised.fill").font(.headline)
                        Text(verbatim: request.params.pretty).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                        if request.method == "item/commandExecution/requestApproval" || request.method == "item/fileChange/requestApproval" {
                            HStack {
                                Button("允许一次") { Task { await store.perform { try await store.approve(request.id, allow: true) } } }
                                Button("拒绝") { Task { await store.perform { try await store.approve(request.id, allow: false) } } }
                            }.disabled(store.busy)
                        } else { InteractiveRequestForm(store: store, request: request) }
                    }
                }
            }.padding()
        }.frame(maxHeight: 200).background(.yellow.opacity(0.08))
    }
}
