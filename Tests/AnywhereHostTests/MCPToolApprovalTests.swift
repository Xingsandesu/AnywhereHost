import Testing
@testable import AnywhereHost

struct MCPToolApprovalTests {
    private var params: JSONValue {
        .object(["mode": .string("form"), "requestedSchema": .object(["type": .string("object"), "properties": .object([:])]),
                 "_meta": .object(["codex_approval_kind": .string("mcp_tool_call"), "persist": .array([.string("session")])])])
    }
    @Test func approvalPreservesEmptyObjectAndAdvertisedScopes() throws {
        let approval = try #require(MCPToolApproval(params))
        #expect(approval.accept()?["content"] == .object([:]))
        #expect(approval.accept(scope: "session")?["_meta"]["persist"] == .string("session"))
        #expect(approval.accept(scope: "always") == nil)
    }
    @Test func ordinaryFormsAndVerificationCannotBeApprovedAsEmptyObjects() {
        var values = params.objectValue
        values["_meta"] = .null
        #expect(MCPToolApproval(.object(values)) == nil)
        values = params.objectValue
        values["mode"] = .string("openai/userVerification")
        #expect(MCPToolApproval(.object(values)) == nil)
        values = params.objectValue
        values["requestedSchema"] = .object(["type": .string("object"), "properties": .object(["name": .object(["type": .string("string")])])])
        #expect(MCPToolApproval(.object(values)) == nil)
    }
}
