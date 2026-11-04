import Foundation
import CryptoKit
import DeviceCheck

// Shared verbatim with the iOS target. Only the signed outer application calls App Attest.
actor NetworkAdmission {
    static let origin = "https://connect.jintongshu.com"
    struct Lease: Codable, Sendable { let token: String; let expiresAt: Double; let relays: [String] }
    struct Challenge: Decodable { let challenge: String?; let attested: Bool? }
    struct Failure: LocalizedError { let message: String; var errorDescription: String? { message } }
    private let role: String
    private var tail: Task<Void, Never>?
    private var leases: [Data: Lease] = [:]
    private let session: URLSession
    init(role: String) {
        self.role = role
        let c = URLSessionConfiguration.ephemeral
        c.tlsMinimumSupportedProtocolVersion = .TLSv13; c.timeoutIntervalForRequest = 20; c.timeoutIntervalForResource = 30
        c.httpShouldSetCookies = false; c.urlCredentialStorage = nil; c.urlCache = nil
        session = URLSession(configuration: c, delegate: AdmissionRedirectPolicy(), delegateQueue: nil)
    }
    func lease(secret: Data) async throws -> Lease {
        let previous = tail
        let task = Task {
            if let previous { await previous.value }
            // Reconnects use the same signed identity. Do not repeat App Attest
            // and three HTTP exchanges while its in-memory lease is still fresh.
            if let lease = self.leases[secret], lease.expiresAt > Date().timeIntervalSince1970 + 360 { return lease }
            let lease = try await self.issue(secret: secret)
            self.leases = self.leases.filter { $0.value.expiresAt > Date().timeIntervalSince1970 }
            self.leases[secret] = lease
            return lease
        }
        tail = Task { _ = try? await task.value }
        return try await task.value
    }
    private func issue(secret: Data) async throws -> Lease {
        let service = DCAppAttestService.shared
        guard service.isSupported else { throw Failure(message: "此设备无法验证官方应用，请使用支持 App Attest 的设备和正式签名版本") }
        let preference = "networkAdmission.v3.\(role)"
        let key: String
        if let existing = UserDefaults.standard.string(forKey: preference) { key = existing }
        else { key = try await service.generateKey(); UserDefaults.standard.set(key, forKey: preference) }
        let enrollment: Challenge = try await post("challenges", ["keyId": key, "role": role, "purpose": "attest"])
        if enrollment.attested != true {
            guard let challenge = enrollment.challenge else { throw Failure(message: "应用验证挑战无效") }
            let attestation: Data
            do { attestation = try await service.attestKey(key, clientDataHash: Data(SHA256.hash(data: Data(challenge.utf8)))) }
            catch { if let e = error as? DCError, e.code != .serverUnavailable { UserDefaults.standard.removeObject(forKey: preference) }; throw error }
            struct Registered: Decodable { let registered: Bool }
            let _: Registered = try await post("attestations", ["keyId": key, "role": role, "challenge": challenge, "attestation": attestation.base64EncodedString()])
        }
        let challenge: Challenge = try await post("challenges", ["keyId": key, "role": role, "purpose": "lease"])
        guard let nonce = challenge.challenge else { throw Failure(message: "接入验证挑战无效") }
        let identity = try Curve25519.Signing.PrivateKey(rawRepresentation: secret)
        let endpoint = identity.publicKey.rawRepresentation.map { String(format: "%02x", $0) }.joined()
        let proof = "Anywhere-Network-Endpoint-v3\n\(Self.origin)\n\(key)\n\(role)\n\(nonce)\n\(endpoint)"
        let bytes = try JSONEncoder().encode(["endpointId": endpoint, "signature": try identity.signature(for: Data(proof.utf8)).base64EncodedString()])
        var message = Data("Anywhere-Network-v3\n\(Self.origin)/v3/network/leases\n\(nonce)\n".utf8); message.append(bytes)
        let assertion: Data
        do { assertion = try await service.generateAssertion(key, clientDataHash: Data(SHA256.hash(data: message))) }
        catch { if let e = error as? DCError, e.code == .invalidKey { UserDefaults.standard.removeObject(forKey: preference) }; throw error }
        return try await post("leases", ["keyId": key, "role": role, "challenge": nonce, "data": bytes.base64EncodedString(), "assertion": assertion.base64EncodedString()])
    }
    private func post<T: Decodable>(_ path: String, _ value: [String: String]) async throws -> T {
        var request = URLRequest(url: URL(string: Self.origin + "/v3/network/" + path)!)
        request.httpMethod = "POST"; request.httpBody = try JSONEncoder().encode(value)
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let (bytes, response) = try await session.bytes(for: request)
        var data = Data()
        for try await byte in bytes { guard data.count < 8192 else { throw Failure(message: "接入服务响应过大") }; data.append(byte) }
        guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
            let error = (try? JSONSerialization.jsonObject(with: data)) as? [String: String]
            throw Failure(message: "官方应用验证未通过（\(error?["error"] ?? "网络不可用")）")
        }
        return try JSONDecoder().decode(T.self, from: data)
    }
}
nonisolated private final class AdmissionRedirectPolicy: NSObject, URLSessionTaskDelegate, Sendable {
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) { completionHandler(nil) }
}
