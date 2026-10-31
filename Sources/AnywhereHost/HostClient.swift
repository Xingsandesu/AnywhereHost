import Foundation
import CryptoKit
import Security

// JSON is retained losslessly across protocol versions. UI-facing models decode only fields they use.
enum JSONValue: Codable, Sendable, Equatable {
    case object([String: JSONValue]), array([JSONValue]), string(String), number(Double), bool(Bool), null
    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let v = try? c.decode(Bool.self) { self = .bool(v) }
        else if let v = try? c.decode(String.self) { self = .string(v) }
        else if let v = try? c.decode(Double.self) { self = .number(v) }
        else if let v = try? c.decode([JSONValue].self) { self = .array(v) }
        else { self = .object(try c.decode([String: JSONValue].self)) }
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .object(let v): try c.encode(v)
        case .array(let v): try c.encode(v)
        case .string(let v): try c.encode(v)
        case .number(let v): try c.encode(v)
        case .bool(let v): try c.encode(v)
        case .null: try c.encodeNil()
        }
    }
    subscript(_ key: String) -> JSONValue { if case .object(let v) = self { return v[key] ?? .null }; return .null }
    var string: String? { if case .string(let v) = self { return v }; return nil }
    var bool: Bool { if case .bool(let v) = self { return v }; return false }
}
struct HostFailure: LocalizedError { let message: String; var errorDescription: String? { message } }

final class PinnedSessionDelegate: NSObject, URLSessionDelegate, URLSessionTaskDelegate, @unchecked Sendable {
    let fingerprint: String
    init(fingerprint: String) { self.fingerprint = fingerprint }
    func urlSession(_ session: URLSession, didReceive challenge: URLAuthenticationChallenge,
                    completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
        guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
              let trust = challenge.protectionSpace.serverTrust,
              let chain = SecTrustCopyCertificateChain(trust) as? [SecCertificate], let leaf = chain.first else {
            completionHandler(.performDefaultHandling, nil); return
        }
        let digest = SHA256.hash(data: SecCertificateCopyData(leaf) as Data).map { String(format: "%02x", $0) }.joined()
        guard digest == fingerprint else { completionHandler(.cancelAuthenticationChallenge, nil); return }
        completionHandler(.useCredential, URLCredential(trust: trust))
    }
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) { completionHandler(nil) }
}
protocol HostConnection: Sendable {
    func request(_ path: String, body: JSONValue?) async throws -> JSONValue
    func pairingLink(endpoint: String) async throws -> String
}

extension HostConnection {
    func request(_ path: String) async throws -> JSONValue { try await request(path, body: nil) }
}

final class HostClient: HostConnection {
    let base: URL
    let token: String
    let fingerprint: String
    private let session: URLSession
    init(stateDirectory: URL) throws {
        token = try String(contentsOf: stateDirectory.appending(path: "admin-key-v2"), encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)
        let pem = try String(contentsOf: stateDirectory.appending(path: "host-cert.pem"), encoding: .utf8)
        let body = pem.components(separatedBy: .newlines).filter { !$0.hasPrefix("-----") }.joined()
        guard let der = Data(base64Encoded: body) else { throw HostFailure(message: "无法读取主机证书") }
        fingerprint = SHA256.hash(data: der).map { String(format: "%02x", $0) }.joined()
        let config = try JSONDecoder().decode(JSONValue.self, from: Data(contentsOf: stateDirectory.appending(path: "config.json")))
        let listen = config["listen"].string ?? "127.0.0.1:9443"
        let port = listen.split(separator: ":").last.map(String.init) ?? "9443"
        base = URL(string: "https://127.0.0.1:\(port)")!
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = 70
        configuration.tlsMinimumSupportedProtocolVersion = .TLSv13
        configuration.urlCredentialStorage = nil; configuration.urlCache = nil
        configuration.httpShouldSetCookies = false
        session = URLSession(configuration: configuration, delegate: PinnedSessionDelegate(fingerprint: fingerprint), delegateQueue: nil)
    }
    func request(_ path: String, body: JSONValue? = nil) async throws -> JSONValue {
        var request = URLRequest(url: base.appending(path: path))
        if let body {
            request.httpMethod = "POST"
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONEncoder().encode(body)
        }
        let (challengeData, challengeResponse) = try await session.data(from: base.appending(path: "v1/security/challenge"))
        let challenge = try JSONDecoder().decode(JSONValue.self, from: challengeData)
        guard (challengeResponse as? HTTPURLResponse)?.statusCode == 200,
              let nonce = challenge["challenge"].string,
              case .number(let serverTime) = challenge["serverTime"] else { throw HostFailure(message: "主机安全握手失败") }
        let timestamp = String(Int64(serverTime))
        let requestID = UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased()
        let components = URLComponents(url: request.url!, resolvingAgainstBaseURL: false)!
        let target = components.percentEncodedPath + (components.percentEncodedQuery.map { "?" + $0 } ?? "")
        let digest = SHA256.hash(data: request.httpBody ?? Data()).map { String(format: "%02x", $0) }.joined()
        let canonical = ["Anywhere-v2", fingerprint, "Anywhere-Admin", request.httpMethod ?? "GET", target, digest, timestamp, requestID, nonce].joined(separator: "\n")
        request.setValue(digest, forHTTPHeaderField: "X-Anywhere-Content-SHA256")
        request.setValue("Anywhere-Admin", forHTTPHeaderField: "Authorization")
        request.setValue(nonce, forHTTPHeaderField: "X-Anywhere-Challenge")
        request.setValue(timestamp, forHTTPHeaderField: "X-Anywhere-Time")
        request.setValue(requestID, forHTTPHeaderField: "X-Anywhere-Nonce")
        request.setValue(Data(HMAC<SHA256>.authenticationCode(for: Data(canonical.utf8), using: SymmetricKey(data: Data(token.utf8)))).base64EncodedString(), forHTTPHeaderField: "X-Anywhere-Signature")
        let (data, response) = try await session.data(for: request)
        let value = try JSONDecoder().decode(JSONValue.self, from: data)
        guard let response = response as? HTTPURLResponse, (200..<300).contains(response.statusCode) else {
            throw HostFailure(message: value["error"].string ?? "主机请求失败")
        }
        return value
    }
    func pairingLink(endpoint: String) async throws -> String {
        guard let url = URL(string: endpoint), url.scheme == "https", url.host != nil, url.user == nil, url.password == nil, url.query == nil, (url.path.isEmpty || url.path == "/"), url.fragment == nil else {
            throw HostFailure(message: "请输入 https://主机:端口 格式的远程地址")
        }
        let ticket = try await request("v1/security/ticket", body: .object([:]))
        guard let secret = ticket["ticket"].string else { throw HostFailure(message: "无法生成配对二维码") }
        var link = URLComponents()
        link.scheme = "anywhere-host"; link.host = "connect"
        link.queryItems = [URLQueryItem(name: "v", value: "2"), URLQueryItem(name: "endpoint", value: endpoint), URLQueryItem(name: "ticket", value: secret), URLQueryItem(name: "certificateSHA256", value: fingerprint)]
        return link.string!
    }
}
