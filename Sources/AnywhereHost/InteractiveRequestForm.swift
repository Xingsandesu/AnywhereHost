import SwiftUI

struct RequestQuestion: Identifiable {
    let id: String
    let prompt: String
    let secret: Bool
    let options: [String]
    init?(_ value: JSONValue) {
        guard let id = value["id"].string else { return nil }
        self.id = id; prompt = value["question"].string ?? value["header"].string ?? id
        secret = value["isSecret"].bool
        options = value["options"].array.compactMap { $0["label"].string }
    }
}
struct InteractiveRequestForm: View {
    var store: HostStore
    let request: PendingRequest
    @State private var answers: [String: String] = [:]
    var body: some View {
        if request.method == "item/tool/requestUserInput" {
            let questions = request.params["questions"].array.compactMap(RequestQuestion.init)
            VStack(alignment: .leading, spacing: 12) {
                ForEach(questions) { question in
                    RequestQuestionField(question: question, answer: Binding(get: { answers[question.id] ?? "" }, set: { answers[question.id] = $0 }))
                }
                Button("提交回答") {
                    let result = JSONValue.object(["answers": .object(answers.mapValues { .object(["answers": .array([.string($0)])]) })])
                    Task { await store.perform { try await store.respond(request.id, result: result) } }
                }.disabled(store.busy || questions.contains { (answers[$0.id] ?? "").isEmpty })
            }
        } else if request.method == "item/permissions/requestApproval" {
            HStack {
                Button("仅本轮允许") {
                    Task { await store.perform { try await store.respond(request.id, result: .object(["permissions": request.params["permissions"], "scope": .string("turn")])) } }
                }
                Button("拒绝权限") {
                    Task { await store.perform { try await store.respond(request.id, result: .object(["permissions": .object([:]), "scope": .string("turn")])) } }
                }
            }.disabled(store.busy)
        } else if let approval = MCPToolApproval(request.params) {
            MCPToolApprovalView(approval: approval) { result in
                Task { await store.perform { try await store.respond(request.id, result: result) } }
            }.disabled(store.busy)
        } else if MCPRequestForm.supports(request.params) {
            MCPRequestForm(params: request.params) { result in
                Task { await store.perform { try await store.respond(request.id, result: result) } }
            }.disabled(store.busy)
        } else {
            // MCP elicitation schemas vary. Do not invent a positive answer to an unknown form.
            VStack(alignment: .leading, spacing: 8) {
                Text("此工具需要额外输入；当前控制台尚不支持该表单。").font(.caption).foregroundStyle(.secondary)
                Button("取消请求") {
                    Task { await store.perform { try await store.respond(request.id, result: .object(["action": .string("cancel"), "content": .null])) } }
                }.disabled(store.busy)
            }
        }
    }
}
struct RequestQuestionField: View {
    let question: RequestQuestion
    @Binding var answer: String
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(verbatim: question.prompt)
            if question.secret { SecureField("回答", text: $answer) }
            else {
                if !question.options.isEmpty {
                    Picker("选择", selection: $answer) {
                        Text("请选择或填写回答").tag("")
                        ForEach(question.options, id: \.self) { Text(verbatim: $0).tag($0) }
                    }
                }
                TextField("回答", text: $answer)
            }
        }
    }
}
