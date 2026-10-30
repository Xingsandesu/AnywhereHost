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
    var array: [JSONValue] { if case .array(let v) = self { return v }; return [] }
    var objectValue: [String: JSONValue] { if case .object(let v) = self { return v }; return [:] }
    var pretty: String { let e = JSONEncoder(); e.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]; return (try? String(decoding: e.encode(self), as: UTF8.self)) ?? "" }
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
final class HostClient: Sendable {
    let base: URL
    let token: String
    let fingerprint: String
    private let session: URLSession
    init(stateDirectory: URL) throws {
        token = try String(contentsOf: stateDirectory.appending(path: "access-token"), encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)
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
        configuration.httpShouldSetCookies = false
        session = URLSession(configuration: configuration, delegate: PinnedSessionDelegate(fingerprint: fingerprint), delegateQueue: nil)
    }
    func request(_ path: String, body: JSONValue? = nil) async throws -> JSONValue {
        var request = URLRequest(url: base.appending(path: path))
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        if let body {
            request.httpMethod = "POST"
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONEncoder().encode(body)
        }
        let (data, response) = try await session.data(for: request)
        let value = try JSONDecoder().decode(JSONValue.self, from: data)
        guard let response = response as? HTTPURLResponse, (200..<300).contains(response.statusCode) else {
            throw HostFailure(message: value["error"].string ?? "主机请求失败")
        }
        return value
    }
    func rpc(_ method: String, _ params: [String: JSONValue] = [:]) async throws -> JSONValue {
        let response = try await request("v1/rpc", body: .object(["method": .string(method), "params": .object(params)]))
        if response["error"] != .null { throw HostFailure(message: response["error"]["message"].string ?? "Codex 请求失败") }
        return response["result"]
    }
    func socket(after: Int, epoch: String?) -> URLSessionWebSocketTask {
        var components = URLComponents(url: base.appending(path: "v1/events"), resolvingAgainstBaseURL: false)!
        components.scheme = "wss"
        components.queryItems = [URLQueryItem(name: "after", value: String(after))]
        if let epoch { components.queryItems?.append(URLQueryItem(name: "epoch", value: epoch)) }
        var request = URLRequest(url: components.url!)
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        return session.webSocketTask(with: request)
    }
    func pairingLink(endpoint: String) throws -> String {
        guard let url = URL(string: endpoint), url.scheme == "https", url.host != nil, url.user == nil, url.password == nil, url.query == nil, url.fragment == nil else {
            throw HostFailure(message: "请输入 https://主机:端口 格式的远程地址")
        }
        var link = URLComponents()
        link.scheme = "anywhere-host"; link.host = "connect"
        link.queryItems = [URLQueryItem(name: "v", value: "1"), URLQueryItem(name: "endpoint", value: endpoint), URLQueryItem(name: "token", value: token), URLQueryItem(name: "certificateSHA256", value: fingerprint)]
        return link.string!
    }
}
