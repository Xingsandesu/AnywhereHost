import Foundation

// 界面仅解码所需字段。
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

protocol HostConnection: Sendable {
    func request(_ method: String, body: JSONValue?) async throws -> JSONValue
    func pairingLink() async throws -> String
}
extension HostConnection {
    func request(_ method: String) async throws -> JSONValue { try await request(method, body: nil) }
}
final class HostClient: HostConnection {
    let stateDirectory: URL
    init(stateDirectory: URL) throws { self.stateDirectory = stateDirectory }
    func request(_ method: String, body: JSONValue? = nil) async throws -> JSONValue {
        let directory = stateDirectory
        return try await Task.detached {
            let process = Process()
            process.executableURL = Bundle.main.bundleURL.appending(path: "Contents/MacOS/anywhere-host")
            process.arguments = ["--control", "--state-dir", directory.path]
            let input = Pipe(), output = Pipe(), errors = Pipe()
            process.standardInput = input; process.standardOutput = output; process.standardError = errors
            try process.run()
            let request: JSONValue = .object(["id": .string(UUID().uuidString), "method": .string(method), "params": body ?? .object([:])])
            try input.fileHandleForWriting.write(contentsOf: JSONEncoder().encode(request))
            try input.fileHandleForWriting.close()
            let bytes = output.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            guard process.terminationStatus == 0 else { throw HostFailure(message: "本机服务不可用") }
            let value = try JSONDecoder().decode(JSONValue.self, from: bytes)
            if let error = value["error"]["message"].string { throw HostFailure(message: error) }
            return value
        }.value
    }
    func pairingLink() async throws -> String {
        let value = try await request("pairing.create")
        return try Self.pairingLink(value, computerName: Host.current().localizedName ?? ProcessInfo.processInfo.hostName)
    }
    static func pairingLink(_ value: JSONValue, computerName: String) throws -> String {
        let name = computerName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let id = value["hostId"].string, let ticket = value["ticket"].string,
              !name.isEmpty else { throw HostFailure(message: "连接信息不完整，请重新启动主机") }
        var link = URLComponents(); link.scheme = "anywhere-host"; link.host = "pair"
        link.queryItems = [URLQueryItem(name: "hostId", value: id), URLQueryItem(name: "hostName", value: name), URLQueryItem(name: "ticket", value: ticket), URLQueryItem(name: "expires", value: String(Int(Date().timeIntervalSince1970) + 120))]
        return link.string!
    }
}
