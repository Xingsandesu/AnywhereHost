import SwiftUI

struct MCPToolApprovalView: View {
    let approval: MCPToolApproval
    let onSend: (JSONValue) -> Void
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(verbatim: approval.title).font(.headline)
            HStack {
                Button("允许一次") { if let value = approval.accept() { onSend(value) } }
                if approval.scopes.contains("session") {
                    Button("本会话允许") { if let value = approval.accept(scope: "session") { onSend(value) } }
                }
                if approval.scopes.contains("always") {
                    Button("始终允许") { if let value = approval.accept(scope: "always") { onSend(value) } }
                }
                Button("拒绝") { onSend(.object(["action": .string("decline"), "content": .null])) }
            }
        }
    }
}
