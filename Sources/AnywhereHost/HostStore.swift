import AppKit
import Foundation
import Observation

struct HostProject: Identifiable, Sendable {
    let id: String; let name: String; let roots: [String]
    init?(_ value: JSONValue) {
        guard let id = value["id"].string else { return nil }
        self.id = id; name = value["name"].string ?? id
        roots = value["roots"].array.compactMap { $0["path"].string }
    }
}
struct HostThread: Identifiable, Sendable {
    let id: String; let title: String; let cwd: String; let status: String
    init?(_ value: JSONValue) {
        guard let id = value["id"].string else { return nil }
        self.id = id; title = value["name"].string ?? value["preview"].string ?? "新会话"
        cwd = value["cwd"].string ?? ""
        switch value["status"]["type"].string {
        case "notLoaded": status = "历史会话"
        case "idle": status = "就绪"
        case "active": status = "运行中"
        case "systemError": status = "需要处理"
        default: status = "等待同步"
        }
    }
}
@MainActor @Observable final class TimelineItem: Identifiable {
    let id: String; let kind: String; var text: String
    init(id: String, kind: String, text: String) { self.id = id; self.kind = kind; self.text = text }
}
struct PendingRequest: Identifiable, Sendable { let id: String; let method: String; let params: JSONValue }

@MainActor @Observable
final class HostStore {
    let process = HostProcess()
    let permissions = HostPermissionCenter()
    var client: HostClient?
    var status: JSONValue = .null
    var tunnel: JSONValue = .null
    var projects: [HostProject] = []
    var threads: [HostThread] = []
    var items: [TimelineItem] = []
    var pending: [PendingRequest] = []
    var capabilities: JSONValue = .null
    var models: [JSONValue] = []
    var permissionProfiles: [JSONValue] = []
    var settings = HostCodexSettings()
    var defaultSettings: HostCodexSettings = {
        guard let data = UserDefaults.standard.data(forKey: "conversationDefaults"),
              let value = try? JSONDecoder().decode(HostCodexSettings.self, from: data) else { return .init() }
        return value
    }() { didSet { if let data = try? JSONEncoder().encode(defaultSettings) { UserDefaults.standard.set(data, forKey: "conversationDefaults") } } }
    var canMutate = false
    private var loadedThread: String?
    private var editedDuringHistory = Set<String>()
    private var pendingDeltas: [String: String] = [:]
    private var deltaTask: Task<Void, Never>?
    var historyError: String?
    var awaitingFirstMessage = false
    func supports(_ method: String) -> Bool { capabilities["methods"].array.contains(.string(method)) }
    var selectedProject: String?
    var selectedThread: String?
    var nextThreadCursor: String?
    var nextHistoryCursor: String?
    private var drafts: [String: String] = [:]
    var draft: String {
        get { selectedThread.flatMap { drafts[$0] } ?? "" }
        set { if let id = selectedThread { drafts[id] = newValue } }
    }
    var error: String?
    var busy = false
    var loadingHistory = false
    var isStarting = false
    var activeTurn: String?
    var context: JSONValue = .null
    var eventCount = 0
    var lastEvent = "等待事件"
    private var socket: URLSessionWebSocketTask?
    private var streamTask: Task<Void, Never>?
    private var pollTask: Task<Void, Never>?
    private var historyGeneration = UUID()

    var connected: Bool { status["connected"].bool }
    var runtimeLabel: String {
        switch status["runtime"]["mode"].string {
        case "sharedCodexProcess": "共享 Codex 进程"
        case "appRuntime": "Codex App 运行时"
        case "cliRuntime": "Codex CLI 运行时"
        default: "尚未连接"
        }
    }
    func perform(_ operation: @escaping @MainActor () async throws -> Void) async {
        guard !busy else { return }
        busy = true; defer { busy = false }
        do { try await operation() } catch is CancellationError {} catch { self.error = error.localizedDescription }
    }
    func start() async {
        guard !isStarting, client == nil else { return }
        isStarting = true; defer { isStarting = false }
        do {
            try process.start()
            for _ in 0..<60 {
                try await Task.sleep(for: .milliseconds(250))
                guard process.isRunning else { throw HostFailure(message: "后端已退出，请查看主机日志。") }
                if let client = try? HostClient(stateDirectory: process.stateDirectory), let status = try? await client.request("v1/status") {
                    self.client = client; self.status = status
                    startEvents(client)
                    pollTask = Task { [weak self] in
                        while !Task.isCancelled {
                            await self?.refresh()
                            try? await Task.sleep(for: .seconds(5))
                        }
                    }
                    return
                }
            }
            throw HostFailure(message: "后端启动超时，请查看主机日志。")
        } catch { self.error = error.localizedDescription }
    }
    func stop() {
        streamTask?.cancel(); pollTask?.cancel(); socket?.cancel(with: .goingAway, reason: nil)
        process.stop(); client = nil; status = .null
    }
    func refresh() async {
        guard let client else { return }
        do {
            status = try await client.request("v1/status")
            tunnel = try await client.request("v1/tunnel")
            capabilities = try await client.request("v1/capabilities")
            if connected {
                var all: [HostProject] = []; var cursor: String?
                repeat {
                    var params: [String: JSONValue] = ["limit": .number(100)]
                    if let cursor { params["cursor"] = .string(cursor) }
                    let result = try await client.rpc("project/list", params)
                    all += result["data"].array.compactMap(HostProject.init)
                    cursor = result["nextCursor"].string
                } while cursor != nil && !Task.isCancelled
                projects = all
                try await loadPending()
            }
        } catch { self.error = error.localizedDescription }
    }
    func loadThreads(more: Bool = false) async {
        guard let client, connected else { return }
        let project = selectedProject
        do {
            var params: [String: JSONValue] = ["limit": .number(50), "sortKey": .string("updated_at")]
            if let project { params["projectId"] = .string(project) }
            if more, let nextThreadCursor { params["cursor"] = .string(nextThreadCursor) }
            let result = try await client.rpc("thread/list", params)
            guard project == selectedProject, !Task.isCancelled else { return }
            let page = result["data"].array.compactMap(HostThread.init)
            if more { let existing = Set(threads.map(\.id)); threads += page.filter { !existing.contains($0.id) } } else { threads = page }
            nextThreadCursor = result["nextCursor"].string
        } catch { self.error = error.localizedDescription }
    }
    func loadHistory(more: Bool = false) async {
        guard let client, let id = selectedThread else { items = []; loadedThread = nil; return }
        if more && nextHistoryCursor == nil { return }
        flushDeltas()
        let generation = UUID(); historyGeneration = generation
        if loadedThread != id {
            items = []; activeTurn = nil; nextHistoryCursor = nil; canMutate = false
            awaitingFirstMessage = false; settings = .init()
            if context["thread"]["id"].string != id { context = .null }
            loadedThread = id
        }
        loadingHistory = true; historyError = nil; editedDuringHistory = []
        let turnAtStart = activeTurn
        defer { if historyGeneration == generation { loadingHistory = false } }
        do {
            async let access = client.request("v1/threads/\(id)/access")
            let turns = try await client.rpc("thread/turns/list", ["threadId": .string(id), "limit": .number(5), "itemsView": .string("notLoaded"), "sortDirection": .string("desc")])
            let empty = turns["historyState"].string == "unmaterialized"
            var p: [String: JSONValue] = ["threadId": .string(id), "limit": .number(60), "sortDirection": .string("desc")]
            if more, let nextHistoryCursor { p["cursor"] = .string(nextHistoryCursor) }
            let result = empty ? JSONValue.object(["data": .array([])]) : try await client.rpc("thread/items/list", p)
            let permission = try await access
            guard selectedThread == id, historyGeneration == generation, !Task.isCancelled else { return }
            canMutate = permission["canMutate"].bool
            awaitingFirstMessage = empty
            if empty { settings = defaultSettings } else { settings.absorb(context) }
            let page = result["data"].array.reversed().compactMap { Self.timelineItem($0["item"]) }
            let existing = Dictionary(items.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
            for item in page where !editedDuringHistory.contains(item.id) {
                existing[item.id]?.text = item.text
            }
            let additions = page.filter { existing[$0.id] == nil }
            if more { items = additions + items }
            else {
                let ids = Set(page.map(\.id))
                if let first = items.firstIndex(where: { ids.contains($0.id) }),
                   let last = items.lastIndex(where: { ids.contains($0.id) }) {
                    let retainedLive = items[first...last].filter { !ids.contains($0.id) && editedDuringHistory.contains($0.id) }
                    items = Array(items[..<first]) + page.map { existing[$0.id] ?? $0 } + retainedLive + items.dropFirst(last + 1)
                } else { items += additions }
            }
            if more || nextHistoryCursor == nil { nextHistoryCursor = result["nextCursor"].string }
            if !more && activeTurn == turnAtStart && !editedDuringHistory.contains("turn") {
                activeTurn = turns["data"].array.first(where: { $0["status"].string == "inProgress" })?["id"].string
            }
        } catch {
            if historyGeneration == generation { historyError = error.localizedDescription }
        }
    }
    private static func timelineItem(_ item: JSONValue) -> TimelineItem? {
        guard let id = item["id"].string, let kind = item["type"].string else { return nil }
        let text: String
        switch kind {
        case "userMessage": text = item["content"].array.compactMap { $0["text"].string }.joined(separator: "\n")
        case "agentMessage": text = item["text"].string ?? ""
        case "reasoning": text = item["summary"].array.compactMap(\.string).joined(separator: "\n")
        case "commandExecution": text = [item["command"].string, item["aggregatedOutput"].string].compactMap { $0 }.joined(separator: "\n")
        default: text = item.pretty
        }
        return TimelineItem(id: id, kind: kind, text: text)
    }
    func createProject() async throws {
        guard let client else { return }
        let picker = NSOpenPanel(); picker.canChooseDirectories = true; picker.canChooseFiles = false; picker.allowsMultipleSelection = false
        guard await picker.begin() == .OK, let url = picker.url else { return }
        let result = try await client.rpc("project/create", ["idempotencyKey": .string(UUID().uuidString), "name": .string(url.lastPathComponent), "roots": .array([.object(["path": .string(url.path)])])])
        selectedProject = result["project"]["id"].string
        await refresh()
    }
    func createThread() async throws {
        guard let client, let project = projects.first(where: { $0.id == selectedProject }), let cwd = project.roots.first else { throw HostFailure(message: "请先选择一个项目") }
        var p: [String: JSONValue] = ["cwd": .string(cwd), "projectId": .string(project.id)]
        let overrides = try defaultSettings.parameters()
        for key in ["model", "approvalPolicy", "permissions", "serviceTier"] { p[key] = overrides[key] }
        if defaultSettings.permissionProfile.isEmpty, defaultSettings.access != .inherit {
            p["sandbox"] = .string(defaultSettings.access == .readOnly ? "read-only" : defaultSettings.access == .workspaceWrite ? "workspace-write" : "danger-full-access")
        }
        let result = try await client.rpc("thread/start", p)
        context = result; selectedThread = result["thread"]["id"].string
        awaitingFirstMessage = true
        await loadThreads()
    }
    func forkThread() async throws {
        guard let client, let id = selectedThread else { return }
        let result = try await client.rpc("thread/fork", ["threadId": .string(id), "excludeTurns": .bool(true)])
        selectedThread = result["thread"]["id"].string; context = result
        await loadThreads()
    }
    func send() async throws {
        guard let client, let id = selectedThread, !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        let text = draft
        guard canMutate else { throw HostFailure(message: "请先分叉只读会话") }
        if !awaitingFirstMessage {
            let resumed = try await client.rpc("thread/resume", ["threadId": .string(id), "excludeTurns": .bool(true)])
            if selectedThread == id { context = resumed; settings.absorb(resumed) }
        }
        var p = awaitingFirstMessage ? try settings.parameters(currentModel: context["model"].string) : [:]
        p["threadId"] = .string(id)
        p["input"] = .array([.object(["type": .string("text"), "text": .string(text)])])
        let result = try await client.rpc("turn/start", p)
        awaitingFirstMessage = false
        if drafts[id] == text { drafts[id] = "" }
        if selectedThread == id { activeTurn = result["turn"]["id"].string }
    }
    func interrupt() async throws {
        guard let client, let id = selectedThread, let activeTurn else { return }
        _ = try await client.rpc("turn/interrupt", ["threadId": .string(id), "turnId": .string(activeTurn)])
    }
    func compact() async throws {
        guard let client, let id = selectedThread else { return }
        _ = try await client.rpc("thread/compact/start", ["threadId": .string(id)])
    }
    func loadPending() async throws {
        guard let client else { return }
        pending = try await client.request("v1/requests").array.compactMap { item in
            guard let id = item["requestId"].string, let method = item["request"]["method"].string else { return nil }
            return PendingRequest(id: id, method: method, params: item["request"]["params"])
        }
    }
    func approve(_ id: String, allow: Bool) async throws {
        guard let client else { return }
        let response = try await client.request("v1/requests/\(id)/respond", body: .object(["result": .object(["decision": .string(allow ? "accept" : "decline")])]))
        if let message = response["error"]["message"].string { throw HostFailure(message: message) }
        try await loadPending()
    }
    func respond(_ id: String, result: JSONValue) async throws {
        guard let client else { return }
        let response = try await client.request("v1/requests/\(id)/respond", body: .object(["result": result]))
        if let message = response["error"]["message"].string { throw HostFailure(message: message) }
        try await loadPending()
    }
    private func startEvents(_ client: HostClient) {
        streamTask?.cancel()
        streamTask = Task { [weak self] in
            var after = 0
            if case .number(let sequence) = self?.status["lastEventSequence"] { after = Int(sequence) }
            var epoch = self?.status["eventEpoch"].string
            while !Task.isCancelled {
                let socket = client.socket(after: after, epoch: epoch); self?.socket = socket; socket.resume()
                do {
                    while !Task.isCancelled {
                        let message = try await socket.receive()
                        let data: Data
                        switch message { case .data(let v): data = v; case .string(let v): data = Data(v.utf8); @unknown default: continue }
                        let event = try JSONDecoder().decode(JSONValue.self, from: data)
                        if event["error"].string != nil { throw HostFailure(message: "事件需要重新同步") }
                        if case .number(let sequence) = event["sequence"] { after = Int(sequence) }
                        epoch = event["epoch"].string
                        self?.consume(event["payload"])
                    }
                } catch {
                    socket.cancel(with: .goingAway, reason: nil)
                    guard !Task.isCancelled else { return }
                    // A failed replay may mean expired cursor/server reset. Reload authoritative state.
                    if let current = try? await client.request("v1/status") {
                        if case .number(let sequence) = current["lastEventSequence"] { after = Int(sequence) }
                        epoch = current["eventEpoch"].string
                    }
                    await self?.loadHistory(); try? await self?.loadPending()
                    try? await Task.sleep(for: .seconds(2))
                }
            }
        }
    }
    private func flushDeltas() {
        deltaTask?.cancel(); deltaTask = nil
        for item in items { if let text = pendingDeltas[item.id] { item.text = text } }
        pendingDeltas = [:]
    }

    func loadSettingsCatalog() async throws {
        guard let client else { return }
        if supports("model/list") { models = try await client.rpc("model/list")["data"].array }
        if supports("permissionProfile/list") {
            var all: [JSONValue] = []; var cursor: String?
            repeat {
                var p: [String: JSONValue] = [:]
                if let cursor { p["cursor"] = .string(cursor) }
                let result = try await client.rpc("permissionProfile/list", p)
                all += result["data"].array
                let next = result["nextCursor"].string
                guard next != cursor || next == nil else { throw HostFailure(message: "权限列表游标没有前进") }
                cursor = next
            } while cursor != nil
            permissionProfiles = all
        }
        if let id = selectedThread, canMutate, !awaitingFirstMessage {
            let result = try await client.rpc("thread/resume", ["threadId": .string(id), "excludeTurns": .bool(true)])
            guard selectedThread == id else { return }
            context = result; settings.absorb(result)
        }
    }
    func applySettings(_ value: HostCodexSettings) async throws {
        guard let client, let id = selectedThread, canMutate else { throw HostFailure(message: "请先分叉只读会话") }
        var p = try value.changes(from: settings, context: context); p["threadId"] = .string(id)
        _ = try await client.rpc("thread/settings/update", p)
        if selectedThread == id { settings = value }
    }
    func nativeAction(_ method: String) async throws {
        guard let client, let id = selectedThread else { return }
        var p: [String: JSONValue] = ["threadId": .string(id)]
        if method == "review/start" { p["delivery"] = .string("inline"); p["target"] = .object(["type": .string("uncommittedChanges")]) }
        if method == "turn/steer" {
            guard let turn = activeTurn else { return }
            p["expectedTurnId"] = .string(turn)
            p["input"] = .array([.object(["type": .string("text"), "text": .string(draft)])])
        }
        if method == "thread/queue/add" {
            p["clientUserMessageId"] = .string(UUID().uuidString)
            p["input"] = .array([.object(["type": .string("text"), "text": .string(draft)])])
        }
        let text = draft
        _ = try await client.rpc(method, p)
        if ["turn/steer", "thread/queue/add"].contains(method), drafts[id] == text { drafts[id] = "" }
    }
    private func consume(_ event: JSONValue) {
        let method = event["method"].string ?? "event"
        eventCount += 1; lastEvent = method
        if method.hasPrefix("host/request/") { Task { try? await loadPending() }; return }
        let params = event["params"]
        guard params["threadId"].string == selectedThread, selectedThread != nil else { return }
        switch method {
        case "turn/started": editedDuringHistory.insert("turn"); activeTurn = params["turn"]["id"].string
        case "turn/completed":
            flushDeltas(); editedDuringHistory.insert("turn")
            if activeTurn == params["turn"]["id"].string { activeTurn = nil }
        case "item/started", "item/completed":
            if let item = Self.timelineItem(params["item"]) {
                pendingDeltas[item.id] = nil; editedDuringHistory.insert(item.id)
                if let existing = items.first(where: { $0.id == item.id }) { existing.text = item.text } else { items.append(item) }
            }
        case "item/agentMessage/delta", "item/commandExecution/outputDelta", "item/plan/delta":
            guard let id = params["itemId"].string, let delta = params["delta"].string,
                  let item = items.first(where: { $0.id == id }) else { return }
            let current = pendingDeltas[id] ?? item.text
            if case .number(let raw) = params["hostTextOffset"], raw >= 0, raw < Double(Int.max) {
                let offset = Int(raw), bytes = current.utf8
                guard offset <= bytes.count else { return }
                let overlap = min(bytes.count - offset, delta.utf8.count)
                guard bytes.dropFirst(offset).prefix(overlap).elementsEqual(delta.utf8.prefix(overlap)),
                      let suffix = String(bytes: delta.utf8.dropFirst(overlap), encoding: .utf8) else { return }
                pendingDeltas[id] = current + suffix
            } else { return }
            editedDuringHistory.insert(id)
            if deltaTask == nil {
                deltaTask = Task { [weak self] in
                    try? await Task.sleep(for: .milliseconds(80))
                    guard !Task.isCancelled else { return }
                    self?.flushDeltas()
                }
            }
        default: break
        }
    }
}
