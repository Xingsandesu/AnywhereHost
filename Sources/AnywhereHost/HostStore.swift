import Foundation
import Observation

@MainActor @Observable
final class HostStore {
    private let admission = NetworkAdmission(role: "host")
    private var admissionTask: Task<Void, Never>?
    let process = HostProcess()
    let permissions = HostPermissionCenter()
    var client: (any HostConnection)?
    private(set) var status: JSONValue = .null
    private(set) var capabilities: JSONValue = .null
    private(set) var desktop: JSONValue = .null
    private(set) var devices: [PairedDevice] = []
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
        case "sharedCodexProcess": return "已连接桌面应用"
        case "appRuntime": return "已连接"
        case "cliRuntime": return "已连接"
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
                   let status = try? await client.request("host.status") {
                    guard lifecycle == generation, !Task.isCancelled else { return }
                    let secret = try Data(contentsOf: process.stateDirectory.appending(path: "identity.key"))
                    let lease = try await admission.lease(secret: secret)
                    guard lifecycle == generation, !Task.isCancelled else { return }
                    _ = try await client.request("admission.install", body: .object(["token": .string(lease.token)]))
                    admissionTask = Task { [weak self] in
                        while !Task.isCancelled {
                            do {
                                try await Task.sleep(for: .seconds(300))
                                guard let self else { return }
                                let renewed = try await self.admission.lease(secret: secret)
                                _ = try await client.request("admission.install", body: .object(["token": .string(renewed.token)]))
                            } catch is CancellationError { return }
                            catch { self?.error = error.localizedDescription }
                        }
                    }
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
        catch { if lifecycle == generation { self.error = error.localizedDescription; process.stop() } }
    }

    func stop() {
        lifecycle = UUID()
        admissionTask?.cancel()
        admissionTask = nil
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
            async let nextStatus = client.request("host.status")
            async let nextCapabilities = client.request("host.capabilities")
            async let nextDevices = client.request("devices.list")
            let (status, capabilities, devices) = try await (nextStatus, nextCapabilities, nextDevices)
            let desktop = capabilities["remoteDesktop"]["supported"].bool
                ? try await client.request("desktop.status") : .null
            guard lifecycle == generation, !Task.isCancelled else { return }
            self.status = status
            self.capabilities = capabilities
            self.desktop = desktop
            self.devices = PairedDevice.decode(devices)
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
            _ = try await client.request("runtime.connect", body: .object([:]))
            await self.refresh()
        }
    }

    func revoke(_ id: String) async {
        await perform {
            guard let client = self.client else { throw HostFailure(message: "主机服务未启动") }
            _ = try await client.request("devices.revoke", body: .object(["deviceId": .string(id)]))
            await self.refresh()
        }
    }

    private func clearStatus() {
        devices = []
        status = .null
        capabilities = .null
        desktop = .null
    }
}
