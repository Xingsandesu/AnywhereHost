import Foundation
import Observation

@MainActor @Observable
final class HostStore {
    let process = HostProcess()
    let permissions = HostPermissionCenter()
    var client: (any HostConnection)?
    private(set) var status: JSONValue = .null
    var tunnel: JSONValue = .null
    private(set) var capabilities: JSONValue = .null
    private(set) var desktop: JSONValue = .null
    var error: String?
    var busy = false
    private(set) var isStarting = false
    @ObservationIgnored private var pollTask: Task<Void, Never>?
    @ObservationIgnored private var lifecycle = UUID()
    @ObservationIgnored private var refreshing = false

    init(client: (any HostConnection)? = nil) { self.client = client }

    var connected: Bool { status["connected"].bool }
    var serviceAvailable: Bool { client != nil && status != .null }
    var runtimeLabel: String {
        guard connected else { return "未连接" }
        switch status["runtime"]["mode"].string {
        case "sharedCodexProcess": return "共享进程"
        case "appRuntime": return "App 运行时"
        case "cliRuntime": return "CLI 运行时"
        default: return "已连接"
        }
    }

    func perform(_ operation: @escaping @MainActor () async throws -> Void) async {
        guard !busy else { return }
        busy = true
        defer { busy = false }
        do { try await operation() }
        catch is CancellationError { }
        catch { self.error = error.localizedDescription }
    }

    func start() async {
        guard !isStarting, client == nil else { return }
        let generation = UUID()
        lifecycle = generation
        isStarting = true
        error = nil
        defer { if lifecycle == generation { isStarting = false } }
        do {
            try process.start()
            for _ in 0..<60 {
                try await Task.sleep(for: .milliseconds(250))
                guard lifecycle == generation else { return }
                guard process.isRunning else { throw HostFailure(message: "主机服务已退出") }
                if let client = try? HostClient(stateDirectory: process.stateDirectory),
                   let status = try? await client.request("v1/status") {
                    guard lifecycle == generation, !Task.isCancelled else { return }
                    self.client = client
                    self.status = status
                    pollTask = Task { [weak self] in
                        while !Task.isCancelled {
                            await self?.refresh()
                            do { try await Task.sleep(for: .seconds(5)) }
                            catch { return }
                        }
                    }
                    return
                }
            }
            throw HostFailure(message: "主机服务启动超时")
        } catch is CancellationError { }
        catch { if lifecycle == generation { self.error = error.localizedDescription } }
    }

    func stop() {
        lifecycle = UUID()
        pollTask?.cancel()
        pollTask = nil
        process.stop()
        client = nil
        isStarting = false
        clearStatus()
    }

    func refresh() async {
        guard let client, !refreshing else { return }
        refreshing = true
        defer { refreshing = false }
        let generation = lifecycle
        do {
            async let nextStatus = client.request("v1/status")
            async let nextTunnel = client.request("v1/tunnel")
            async let nextCapabilities = client.request("v1/capabilities")
            let (status, tunnel, capabilities) = try await (nextStatus, nextTunnel, nextCapabilities)
            let desktop = capabilities["remoteDesktop"]["supported"].bool
                ? try await client.request("v1/desktop") : .null
            guard lifecycle == generation, !Task.isCancelled else { return }
            self.status = status
            self.tunnel = tunnel
            self.capabilities = capabilities
            self.desktop = desktop
        } catch is CancellationError { }
        catch {
            guard lifecycle == generation, !Task.isCancelled else { return }
            clearStatus()
            self.error = error.localizedDescription
        }
    }

    func reconnectRuntime() async {
        await perform {
            guard let client = self.client else { throw HostFailure(message: "主机服务未启动") }
            _ = try await client.request("v1/runtime/connect", body: .object([:]))
            await self.refresh()
        }
    }

    private func clearStatus() {
        status = .null
        tunnel = .null
        capabilities = .null
        desktop = .null
    }
}
