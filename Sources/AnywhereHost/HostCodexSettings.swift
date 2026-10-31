import Foundation

nonisolated enum CodexAccess: String, Codable, CaseIterable, Identifiable, Sendable {
    case inherit, readOnly, workspaceWrite, dangerFullAccess
    var id: String { rawValue }
    var title: String {
        switch self {
        case .inherit: "主机默认"
        case .readOnly: "只读"
        case .workspaceWrite: "工作区权限"
        case .dangerFullAccess: "全部权限"
        }
    }
}

nonisolated struct HostCodexSettings: Codable, Equatable, Sendable {
    var model = ""
    var effort = ""
    var plan: Bool?
    var access: CodexAccess = .inherit
    var approval = ""
    var network: Bool?
    var permissionProfile = ""
    var summary = ""
    var serviceTier = ""

    /// Only explicitly selected overrides are sent. Full access is an atomic pair.
    func parameters(currentModel: String? = nil) throws -> [String: JSONValue] {
        var p: [String: JSONValue] = [:]
        if !model.isEmpty { p["model"] = .string(model) }
        if !effort.isEmpty { p["effort"] = .string(effort) }
        if !summary.isEmpty { p["summary"] = .string(summary) }
        if !serviceTier.isEmpty { p["serviceTier"] = serviceTier == "default" ? .null : .string(serviceTier) }
        if !permissionProfile.isEmpty {
            p["permissions"] = .string(permissionProfile)
        } else if access != .inherit {
            var sandbox: [String: JSONValue] = ["type": .string(access.rawValue)]
            if access != .dangerFullAccess, let network { sandbox["networkAccess"] = .bool(network) }
            p["sandboxPolicy"] = .object(sandbox)
        }
        if access == .dangerFullAccess && permissionProfile.isEmpty {
            p["approvalPolicy"] = .string("never")
        } else if !approval.isEmpty { p["approvalPolicy"] = .string(approval) }
        if let plan {
            let selectedModel = model.isEmpty ? currentModel ?? "" : model
            guard !selectedModel.isEmpty else { throw HostFailure(message: "切换计划模式前请选择模型") }
            p["collaborationMode"] = .object([
                "mode": .string(plan ? "plan" : "default"),
                "settings": .object(["model": .string(selectedModel), "reasoning_effort": effort.isEmpty ? .null : .string(effort), "developer_instructions": .null])
            ])
        }
        return p
    }

    func changes(from old: Self, context: JSONValue) throws -> [String: JSONValue] {
        let full = try parameters(currentModel: context["model"].string)
        var patch: [String: JSONValue] = [:]
        for (changed, key) in [(model != old.model, "model"), (effort != old.effort, "effort"),
                               (summary != old.summary, "summary"), (serviceTier != old.serviceTier, "serviceTier"),
                               (approval != old.approval, "approvalPolicy")] where changed {
            patch[key] = full[key]
        }
        if plan != old.plan || (plan != nil && (model != old.model || effort != old.effort)) {
            patch["collaborationMode"] = full["collaborationMode"]
        }
        if permissionProfile != old.permissionProfile || access != old.access || network != old.network {
            patch["permissions"] = full["permissions"]
            if let sandbox = full["sandboxPolicy"] {
                var policy = sandbox.objectValue
                if access == old.access, context["sandbox"]["type"].string == access.rawValue {
                    policy = context["sandbox"].objectValue.merging(policy) { _, new in new }
                }
                patch["sandboxPolicy"] = .object(policy)
            }
            patch["approvalPolicy"] = full["approvalPolicy"]
        }
        return patch
    }

    mutating func absorb(_ value: JSONValue) {
        if let v = value["model"].string { model = v }
        if let v = value["reasoningEffort"].string ?? value["effort"].string { effort = v }
        if let v = value["approvalPolicy"].string { approval = v }
        if let v = CodexAccess(rawValue: value["sandboxPolicy"]["type"].string ?? value["sandbox"]["type"].string ?? "") { access = v }
        if let v = value["collaborationMode"]["mode"].string { plan = v == "plan" }
        if let v = value["serviceTier"].string { serviceTier = v }
        let policy = value["sandboxPolicy"] != .null ? value["sandboxPolicy"] : value["sandbox"]
        if case .bool(let v) = policy["networkAccess"] { network = v }
        if let v = value["permissions"].string { permissionProfile = v }
        else if let v = value["activePermissionProfile"]["id"].string { permissionProfile = v.hasPrefix(":") ? "" : v }
        if let v = value["summary"].string { summary = v }
    }

    mutating func selectAccess(_ value: CodexAccess) {
        access = value; permissionProfile = ""
        approval = value == .dangerFullAccess ? "never" : value == .inherit ? "" : "on-request"
        network = value == .inherit ? nil : false
    }
}

nonisolated struct RemotePermissionProfile: Identifiable, Equatable, Sendable {
    let id: String
    let description: String
    let allowed: Bool
    init?(_ value: JSONValue) {
        guard let id = value["id"].string else { return nil }
        self.id = id; description = value["description"].string ?? id; allowed = value["allowed"].bool
    }
}
