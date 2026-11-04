import Foundation
import Testing
@testable import AnywhereHost

@MainActor struct HostStoreTests {
    @Test func dashboardOnlyReadsHostResources() async {
        let connection = StatusConnection()
        let store = HostStore(client: connection)
        await store.refresh()
        #expect(store.serviceAvailable)
        #expect(store.connected)
        #expect(store.devices.map(\.id) == ["phone"])
        #expect(await connection.requestedPaths() == Set(["host.status", "host.capabilities", "desktop.status", "devices.list"]))
        #expect(await connection.sentBodies() == 0)
    }

    @Test func failedRefreshClearsPreviouslyHealthyState() async {
        let connection = StatusConnection()
        let store = HostStore(client: connection)
        await store.refresh()
        await connection.failRequests()
        await store.refresh()
        #expect(!store.serviceAvailable)
        #expect(!store.connected)
        #expect(store.devices.isEmpty)
        #expect(store.desktop == .null)
        #expect(store.error != nil)
    }

    @Test func lateStatusCannotRestartAStoppedHost() async {
        let connection = StatusConnection(holdStatus: true)
        let store = HostStore(client: connection)
        let refresh = Task { await store.refresh() }
        await connection.waitForStatusRequest()
        store.stop()
        await connection.releaseStatus()
        await refresh.value
        #expect(store.client == nil)
        #expect(!store.serviceAvailable)
        #expect(store.status == .null)
        #expect(store.devices.isEmpty)
        #expect(store.desktop == .null)
    }
}

private actor StatusConnection: HostConnection {
    private var paths = Set<String>()
    private var bodies = 0
    private var fails = false
    private var holdStatus: Bool
    private var statusGate: CheckedContinuation<Void, Never>?
    private var statusObserver: CheckedContinuation<Void, Never>?

    init(holdStatus: Bool = false) { self.holdStatus = holdStatus }
    nonisolated func pairingLink() async throws -> String { throw HostFailure(message: "Unavailable in fixture") }
    func requestedPaths() -> Set<String> { paths }
    func sentBodies() -> Int { bodies }
    func failRequests() { fails = true }
    func waitForStatusRequest() async {
        if statusGate != nil { return }
        await withCheckedContinuation { statusObserver = $0 }
    }
    func releaseStatus() {
        holdStatus = false
        statusGate?.resume()
        statusGate = nil
    }
    func request(_ path: String, body: JSONValue?) async throws -> JSONValue {
        paths.insert(path)
        if body != nil { bodies += 1 }
        if fails { throw HostFailure(message: "Disconnected") }
        if path == "host.status", holdStatus {
            await withCheckedContinuation {
                statusGate = $0
                statusObserver?.resume()
                statusObserver = nil
            }
        }
        switch path {
        case "host.status": return .object(["connected": .bool(true)])
        case "host.capabilities": return .object(["remoteDesktop": .object(["supported": .bool(true)])])
        case "devices.list": return .object(["devices": .array([.string("phone")]), "connections": .object([:])])
        case "desktop.status": return .object(["enabled": .bool(true), "reachable": .bool(true)])
        default: throw HostFailure(message: "Unexpected client operation")
        }
    }
}
