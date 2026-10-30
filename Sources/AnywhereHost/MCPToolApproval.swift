import Foundation

/// Recognize Codex's explicit tool-approval metadata, not arbitrary empty MCP forms.
struct MCPToolApproval {
    let title: String
    let scopes: [String]
    init?(_ params: JSONValue) {
        guard params["mode"].string == "form",
              params["_meta"]["codex_approval_kind"].string == "mcp_tool_call",
              params["requestedSchema"]["type"].string == "object",
              case .object(let properties) = params["requestedSchema"]["properties"], properties.isEmpty,
              params["requestedSchema"]["required"].array.isEmpty else { return nil }
        title = params["_meta"]["tool_title"].string ?? params["message"].string ?? "工具调用"
        scopes = params["_meta"]["persist"].array.compactMap(\.string).filter { ["session", "always"].contains($0) }
    }
    func accept(scope: String? = nil) -> JSONValue? {
        var value: [String: JSONValue] = ["action": .string("accept"), "content": .object([:])]
        if let scope {
            guard scopes.contains(scope) else { return nil }
            value["_meta"] = .object(["persist": .string(scope)])
        }
        return .object(value)
    }
}
