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
struct TimelineItem: Identifiable, Sendable { let id: String; let kind: String; var text: String }
struct PendingRequest: Identifiable, Sendable { let id: String; let method: String; let params: JSONValue }

@MainActor @Observable
final class HostStore {
    let process = HostProcess()
    var client: HostClient?
    var status: JSONValue = .null
    var tunnel: JSONValue = .null
    var projects: [HostProject] = []
    var threads: [HostThread] = []
    var items: [TimelineItem] = []
    var pending: [PendingRequest] = []
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
        guard let client, let id = selectedThread else { items = []; return }
        let generation = UUID(); historyGeneration = generation
        loadingHistory = true
        if !more {
            items = []; activeTurn = nil
            if context["thread"]["id"].string != id { context = .null }
        }
        defer { if historyGeneration == generation { loadingHistory = false } }
        do {
            var params: [String: JSONValue] = ["threadId": .string(id), "limit": .number(20), "itemsView": .string("full"), "sortDirection": .string("desc")]
            if more, let nextHistoryCursor { params["cursor"] = .string(nextHistoryCursor) }
            let result = try await client.rpc("thread/turns/list", params)
            guard selectedThread == id, historyGeneration == generation, !Task.isCancelled else { return }
            let turns = result["data"].array
            let page = turns.reversed().flatMap { turn in turn["items"].array.compactMap(Self.timelineItem) }
            let known = Set(items.map(\.id))
            if more { items = page.filter { !known.contains($0.id) } + items } else { items = page }
            nextHistoryCursor = result["nextCursor"].string
            activeTurn = turns.first(where: { $0["status"].string == "inProgress" })?["id"].string
        } catch { if historyGeneration == generation { self.error = error.localizedDescription } }
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
        let result = try await client.rpc("thread/start", ["cwd": .string(cwd), "projectId": .string(project.id)])
        context = result; selectedThread = result["thread"]["id"].string
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
        let resumed = try await client.rpc("thread/resume", ["threadId": .string(id), "excludeTurns": .bool(true)])
        if selectedThread == id { context = resumed }
        let result = try await client.rpc("turn/start", ["threadId": .string(id), "input": .array([.object(["type": .string("text"), "text": .string(text)])])])
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
    private func consume(_ event: JSONValue) {
        let method = event["method"].string ?? "event"
        eventCount += 1; lastEvent = method
        if method.hasPrefix("host/request/") { Task { try? await loadPending() }; return }
        let params = event["params"]
        guard params["threadId"].string == selectedThread, selectedThread != nil else { return }
        switch method {
        case "turn/started": activeTurn = params["turn"]["id"].string
        case "turn/completed": activeTurn = nil; Task { await loadHistory() }
        case "item/started", "item/completed":
            if let item = Self.timelineItem(params["item"]) {
                if let index = items.firstIndex(where: { $0.id == item.id }) { items[index] = item } else { items.append(item) }
            }
        case "item/agentMessage/delta":
            guard let id = params["itemId"].string, let delta = params["delta"].string else { return }
            if let index = items.firstIndex(where: { $0.id == id }) { items[index].text += delta }
            else { items.append(TimelineItem(id: id, kind: "agentMessage", text: delta)) }
        default: break
        }
    }
}
