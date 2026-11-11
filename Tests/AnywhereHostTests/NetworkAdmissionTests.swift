import CryptoKit
import DeviceCheck
import Foundation
import Testing
@testable import AnywhereHost

struct NetworkAdmissionTests {
    @Test func rebootFailureRepairsKeyAndNextLaunchReusesIt() async throws {
        let fixture = Fixture()
        defer { fixture.clean() }
        let server = AdmissionFixture(mode: .staleKey)
        let first = fixture.admission(server)
        #expect(try await first.lease(secret: fixture.secret).token == "verified")
        let second = fixture.admission(server)
        #expect(try await second.lease(secret: fixture.secret).token == "verified")
        #expect(await server.generated == 1)
        #expect(await server.attestations == 1)
        #expect(await server.oldAssertions == 4)
        #expect(fixture.savedKey == "new-key")
    }

    @Test func transientFailureKeepsExistingKey() async throws {
        let fixture = Fixture()
        defer { fixture.clean() }
        let server = AdmissionFixture(mode: .transient)
        _ = try await fixture.admission(server).lease(secret: fixture.secret)
        #expect(await server.generated == 0)
        #expect(await server.oldAssertions == 2)
        #expect(fixture.savedKey == "old-key")
    }

    @Test func invalidKeyRepairsInSameRequest() async throws {
        let fixture = Fixture()
        defer { fixture.clean() }
        let server = AdmissionFixture(mode: .invalidKey)
        _ = try await fixture.admission(server).lease(secret: fixture.secret)
        #expect(await server.generated == 1)
        #expect(await server.oldAssertions == 1)
    }

    @Test func permanentFailureHasBoundedRecovery() async throws {
        let fixture = Fixture()
        defer { fixture.clean() }
        let server = AdmissionFixture(mode: .alwaysFails)
        await #expect(throws: NetworkAdmission.Failure.self) {
            try await fixture.admission(server).lease(secret: fixture.secret)
        }
        #expect(await server.generated == 1)
        #expect(await server.assertions == 8)
        #expect(await server.leases == 0)
    }

    @Test func enrollmentRetriesSameKeyAndChallenge() async throws {
        let fixture = Fixture(key: nil)
        defer { fixture.clean() }
        let server = AdmissionFixture(mode: .attestationUnavailable)
        _ = try await fixture.admission(server).lease(secret: fixture.secret)
        #expect(await server.generated == 1)
        #expect(await server.attestationInputs.count == 3)
        #expect(await Set(server.attestationInputs).count == 1)
    }

    @Test func serverRejectionNeverRotatesKey() async throws {
        let fixture = Fixture()
        defer { fixture.clean() }
        let server = AdmissionFixture(mode: .rejected)
        await #expect(throws: NetworkAdmission.Failure.self) {
            try await fixture.admission(server).lease(secret: fixture.secret)
        }
        #expect(await server.generated == 0)
        #expect(fixture.savedKey == "old-key")
    }

    @Test func lostEnrollmentResponseRecoversWithoutAttestingAgain() async throws {
        let fixture = Fixture(key: nil)
        defer { fixture.clean() }
        let server = AdmissionFixture(mode: .lostEnrollmentResponse)
        let admission = fixture.admission(server)
        _ = try await admission.lease(secret: fixture.secret)
        _ = try await fixture.admission(server).lease(secret: fixture.secret)
        #expect(await server.generated == 1)
        #expect(await server.attestations == 1)
    }

    @Test func lostLeaseResponseUsesFreshChallengeAndAssertion() async throws {
        let fixture = Fixture()
        defer { fixture.clean() }
        let server = AdmissionFixture(mode: .lostLeaseResponse)
        _ = try await fixture.admission(server).lease(secret: fixture.secret)
        #expect(await server.leases == 2)
        #expect(await server.assertions == 2)
        #expect(await Set(server.leaseChallenges).count == 2)
        #expect(await server.generated == 0)
    }

    @Test func cancellationStopsRetryAndKeyReplacement() async throws {
        let fixture = Fixture()
        defer { fixture.clean() }
        let server = AdmissionFixture(mode: .staleKey)
        let gate = PauseGate()
        let admission = fixture.admission(server, pause: { _ in
            await gate.arrive()
            try await Task.sleep(for: .seconds(60))
        })
        let task = Task { try await admission.lease(secret: fixture.secret) }
        await gate.wait()
        task.cancel()
        await #expect(throws: CancellationError.self) { try await task.value }
        #expect(await server.generated == 0)
        #expect(await server.oldAssertions == 1)
        #expect(fixture.savedKey == "old-key")
    }

    @Test func concurrentRequestsSerializeKeyRepairAndCounters() async throws {
        let fixture = Fixture()
        defer { fixture.clean() }
        let server = AdmissionFixture(mode: .staleKey)
        let admission = fixture.admission(server)
        async let a = admission.lease(secret: fixture.secret)
        async let b = admission.lease(secret: fixture.secret)
        _ = try await (a, b)
        #expect(await server.generated == 1)
        #expect(await server.leases == 2)
    }
}

private struct Fixture {
    let suite = "AnywhereHostTests.Admission.\(UUID().uuidString)"
    let secret = Curve25519.Signing.PrivateKey().rawRepresentation
    var defaults: UserDefaults { UserDefaults(suiteName: suite)! }
    var savedKey: String? { defaults.string(forKey: "networkAdmission.v3.host") }
    init(key: String? = "old-key") { defaults.set(key, forKey: "networkAdmission.v3.host") }
    func clean() { defaults.removePersistentDomain(forName: suite) }
    func admission(_ server: AdmissionFixture,
                   pause: @escaping @Sendable (Duration) async throws -> Void = { _ in }) -> NetworkAdmission {
        NetworkAdmission(role: "host", service: .init(
            isSupported: { true }, generateKey: { await server.generate() },
            attestKey: { try await server.attest($0, $1) },
            generateAssertion: { try await server.assertion($0, $1) }), defaultsSuite: suite,
            pause: pause, request: { try await server.post($0, $1) })
    }
}

private actor AdmissionFixture {
    enum Mode { case staleKey, transient, invalidKey, alwaysFails, attestationUnavailable, rejected, lostEnrollmentResponse, lostLeaseResponse }
    let mode: Mode
    var generated = 0
    var oldAssertions = 0
    var assertions = 0
    var attestations = 0
    var leases = 0
    var leaseChallenges: [String] = []
    var registered = Set(["old-key"])
    var attestationInputs: [String] = []
    init(mode: Mode) { self.mode = mode }
    func generate() -> String { generated += 1; return "new-key" }
    func attest(_ key: String, _ hash: Data) throws -> Data {
        attestations += 1
        attestationInputs.append(key + hash.base64EncodedString())
        if mode == .attestationUnavailable, attestations < 3 { throw DCError(.serverUnavailable) }
        return Data("attestation".utf8)
    }
    func assertion(_ key: String, _ hash: Data) throws -> Data {
        assertions += 1
        if key == "old-key" {
            oldAssertions += 1
            if mode == .invalidKey { throw DCError(.invalidKey) }
            if mode == .staleKey || (mode == .transient && oldAssertions == 1) {
                throw DCError(.unknownSystemFailure)
            }
        }
        if mode == .alwaysFails { throw DCError(.unknownSystemFailure) }
        return Data("assertion".utf8)
    }
    func post(_ path: String, _ value: [String: String]) throws -> Data {
        let response: [String: Any]
        switch path {
        case "challenges":
            response = value["purpose"] == "attest" && registered.contains(value["keyId"]!)
                ? ["attested": true] : ["challenge": UUID().uuidString]
        case "attestations":
            registered.insert(value["keyId"]!)
            if mode == .lostEnrollmentResponse { throw URLError(.networkConnectionLost) }
            response = ["registered": true]
        case "leases":
            if mode == .rejected { throw NetworkAdmission.Failure(message: "AppNotAuthorized") }
            leases += 1
            leaseChallenges.append(value["challenge"]!)
            if mode == .lostLeaseResponse, leases == 1 { throw URLError(.networkConnectionLost) }
            response = ["token": "verified", "expiresAt": 9999999999.0, "relays": ["https://relay.example/"]]
        default: throw NetworkAdmission.Failure(message: "Unexpected request")
        }
        return try JSONSerialization.data(withJSONObject: response)
    }
}

private actor PauseGate {
    var arrived = false
    var observer: CheckedContinuation<Void, Never>?
    func arrive() { arrived = true; observer?.resume(); observer = nil }
    func wait() async { if !arrived { await withCheckedContinuation { observer = $0 } } }
}
