import Foundation
import CryptoKit
import DeviceCheck
import OSLog

// Shared verbatim with the iOS target. Only the signed outer application calls App Attest.
actor NetworkAdmission {
    static let origin = "https://connect.jintongshu.com"
    struct Lease: Codable, Sendable { let token: String; let expiresAt: Double; let relays: [String] }
    struct Challenge: Decodable { let challenge: String?; let attested: Bool? }
    struct Failure: LocalizedError { let message: String; var errorDescription: String? { message } }
    struct AppAttest: Sendable {
        var isSupported: @Sendable () -> Bool
        var generateKey: @Sendable () async throws -> String
        var attestKey: @Sendable (String, Data) async throws -> Data
        var generateAssertion: @Sendable (String, Data) async throws -> Data
        static let live = AppAttest(
            isSupported: { DCAppAttestService.shared.isSupported },
            generateKey: { try await DCAppAttestService.shared.generateKey() },
            attestKey: { try await DCAppAttestService.shared.attestKey($0, clientDataHash: $1) },
            generateAssertion: { try await DCAppAttestService.shared.generateAssertion($0, clientDataHash: $1) })
    }
    private struct ServiceFailure: LocalizedError {
        let status: Int
        let code: String
        var errorDescription: String? { "官方应用验证未通过（\(code)）" }
    }
    private static let logger = Logger(subsystem: "com.jintongshu.AnywhereHost", category: "NetworkAdmission")
    private let service: AppAttest
    private let defaults: UserDefaults
    private let pause: @Sendable (Duration) async throws -> Void
    private let requestOverride: (@Sendable (String, [String: String]) async throws -> Data)?
    private let role: String
    private var tail: Task<Void, Never>?
    private let session: URLSession
    init(role: String, service: AppAttest = .live, defaultsSuite: String? = nil,
         pause: @escaping @Sendable (Duration) async throws -> Void = { try await Task.sleep(for: $0) },
         request: (@Sendable (String, [String: String]) async throws -> Data)? = nil) {
        self.role = role
        self.service = service
        self.defaults = defaultsSuite.flatMap(UserDefaults.init(suiteName:)) ?? .standard
        self.pause = pause
        self.requestOverride = request
        let c = URLSessionConfiguration.ephemeral
        c.tlsMinimumSupportedProtocolVersion = .TLSv13; c.timeoutIntervalForRequest = 20; c.timeoutIntervalForResource = 30
        c.httpShouldSetCookies = false; c.urlCredentialStorage = nil; c.urlCache = nil
        session = URLSession(configuration: c, delegate: AdmissionRedirectPolicy(), delegateQueue: nil)
    }
    func lease(secret: Data) async throws -> Lease {
        let previous = tail
        let task = Task {
            if let previous { await previous.value }
            try Task.checkCancellation()
            return try await self.issue(secret: secret)
        }
        tail = Task { _ = try? await task.value }
        return try await withTaskCancellationHandler {
            let lease = try await task.value
            try Task.checkCancellation()
            return lease
        } onCancel: { task.cancel() }
    }
    private func issue(secret: Data) async throws -> Lease {
        guard service.isSupported() else { throw Failure(message: "此设备无法验证官方应用，请使用支持 App Attest 的设备和正式签名版本") }
        let preference = "networkAdmission.v3.\(role)"
        // One repair per request. A rejected server proof never triggers key rotation.
        for repair in 0...1 {
            try Task.checkCancellation()
            let key: String
            if let existing = defaults.string(forKey: preference) { key = existing }
            else {
                key = try await retry { try await service.generateKey() }
                defaults.set(key, forKey: preference)
            }
            do { return try await issue(secret: secret, key: key) }
            catch {
                try Task.checkCancellation()
                if repair == 0, Self.needsNewKey(error) {
                    Self.logger.notice("Replacing unusable App Attest key after bounded retries")
                    defaults.removeObject(forKey: preference)
                    continue
                }
                if let dc = error as? DCError {
                    throw Failure(message: "系统应用验证暂时不可用，请稍后重试（DeviceCheck \(dc.code.rawValue)）")
                }
                throw error
            }
        }
        throw Failure(message: "应用验证恢复失败，请稍后重试")
    }
    private func issue(secret: Data, key: String) async throws -> Lease {
        let enrollment: Challenge = try await post("challenges", ["keyId": key, "role": role, "purpose": "attest"])
        if enrollment.attested != true {
            guard let challenge = enrollment.challenge else { throw Failure(message: "应用验证挑战无效") }
            let hash = Data(SHA256.hash(data: Data(challenge.utf8)))
            // Apple requires the same key and challenge hash when retrying serverUnavailable.
            let attestation = try await retry { try await service.attestKey(key, hash) }
            struct Registered: Decodable { let registered: Bool }
            do {
                let registration: Registered = try await post("attestations", ["keyId": key, "role": role, "challenge": challenge, "attestation": attestation.base64EncodedString()])
                guard registration.registered else { throw Failure(message: "应用验证登记未完成") }
            } catch {
                guard Self.isNetworkFailure(error) else { throw error }
                // The server may have committed enrollment before the response was lost.
                let state: Challenge = try await post("challenges", ["keyId": key, "role": role, "purpose": "attest"])
                guard state.attested == true else { throw error }
            }
        }
        return try await retry(when: Self.isNetworkFailure) { try await requestLease(secret: secret, key: key) }
    }
    private func requestLease(secret: Data, key: String) async throws -> Lease {
        let challenge: Challenge = try await post("challenges", ["keyId": key, "role": role, "purpose": "lease"])
        guard let nonce = challenge.challenge else { throw Failure(message: "接入验证挑战无效") }
        let identity = try Curve25519.Signing.PrivateKey(rawRepresentation: secret)
        let endpoint = identity.publicKey.rawRepresentation.map { String(format: "%02x", $0) }.joined()
        let proof = "Anywhere-Network-Endpoint-v3\n\(Self.origin)\n\(key)\n\(role)\n\(nonce)\n\(endpoint)"
        let bytes = try JSONEncoder().encode(["endpointId": endpoint, "signature": try identity.signature(for: Data(proof.utf8)).base64EncodedString()])
        var message = Data("Anywhere-Network-v3\n\(Self.origin)/v3/network/leases\n\(nonce)\n".utf8); message.append(bytes)
        let hash = Data(SHA256.hash(data: message))
        let assertion = try await retry { try await service.generateAssertion(key, hash) }
        return try await post("leases", ["keyId": key, "role": role, "challenge": nonce, "data": bytes.base64EncodedString(), "assertion": assertion.base64EncodedString()])
    }
    private static func needsNewKey(_ error: Error) -> Bool {
        guard let dc = error as? DCError else { return false }
        if dc.code == .invalidKey { return true }
        #if os(macOS)
        // On macOS a persisted key can remain unusable after reboot with error 0.
        // Repair only after retrying that key; never rotate for a network/server error.
        return dc.code == .unknownSystemFailure
        #else
        return false
        #endif
    }
    private func retry<T>(when shouldRetry: (Error) -> Bool = NetworkAdmission.isTransient, _ operation: () async throws -> T) async throws -> T {
        for attempt in 0...3 {
            try Task.checkCancellation()
            do {
                let result = try await operation()
                try Task.checkCancellation()
                return result
            } catch {
                try Task.checkCancellation()
                guard attempt < 3, shouldRetry(error) else { throw error }
                let ns = error as NSError
                Self.logger.info("Retrying application verification: domain=\(ns.domain, privacy: .public) code=\(ns.code) attempt=\(attempt + 1)")
                try await pause(.seconds(1 << attempt))
            }
        }
        throw CancellationError()
    }
    private static func isTransient(_ error: Error) -> Bool {
        if let dc = error as? DCError {
            return dc.code == .unknownSystemFailure || dc.code == .serverUnavailable
        }
        return isNetworkFailure(error)
    }
    private static func isNetworkFailure(_ error: Error) -> Bool {
        if let url = error as? URLError {
            return [.timedOut, .cannotFindHost, .cannotConnectToHost, .networkConnectionLost,
                    .dnsLookupFailed, .notConnectedToInternet].contains(url.code)
        }
        if let http = error as? ServiceFailure { return [502, 503, 504].contains(http.status) }
        return false
    }
    private func post<T: Decodable>(_ path: String, _ value: [String: String]) async throws -> T {
        let operation = {
            if let requestOverride = self.requestOverride { return try await requestOverride(path, value) }
            return try await self.send(path, value)
        }
        // Mutations consume their challenge: recover at the protocol level, never replay them.
        let data = try await path == "challenges" ? retry(operation) : operation()
        return try JSONDecoder().decode(T.self, from: data)
    }
    private func send(_ path: String, _ value: [String: String]) async throws -> Data {
        var request = URLRequest(url: URL(string: Self.origin + "/v3/network/" + path)!)
        request.httpMethod = "POST"; request.httpBody = try JSONEncoder().encode(value)
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let (bytes, response) = try await session.bytes(for: request)
        var data = Data()
        for try await byte in bytes { guard data.count < 8192 else { throw Failure(message: "接入服务响应过大") }; data.append(byte) }
        guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
            let error = (try? JSONSerialization.jsonObject(with: data)) as? [String: String]
            throw ServiceFailure(status: (response as? HTTPURLResponse)?.statusCode ?? 0, code: error?["error"] ?? "网络不可用")
        }
        return data
    }
}
nonisolated private final class AdmissionRedirectPolicy: NSObject, URLSessionTaskDelegate, Sendable {
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) { completionHandler(nil) }
}
