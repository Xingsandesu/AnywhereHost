import SwiftUI

struct MCPRequestForm: View {
    static func supports(_ params: JSONValue) -> Bool {
        if params["mode"].string == "url" {
            guard let value = params["url"].string, let url = URL(string: value) else { return false }
            return ["https", "http"].contains(url.scheme?.lowercased() ?? "") && url.host != nil
        }
        guard params["mode"].string == "form", params["requestedSchema"]["type"].string == "object",
              case .object(let properties) = params["requestedSchema"]["properties"], !properties.isEmpty else { return false }
        return properties.values.allSatisfy { ["string", "boolean", "integer", "number"].contains($0["type"].string ?? "") }
    }
    let params: JSONValue
    let onSend: (JSONValue) -> Void
    @State private var values: [String: String] = [:]
    private var fields: [String] { params["requestedSchema"]["properties"].objectValue.keys.sorted() }
    private var required: [String] { params["requestedSchema"]["required"].array.compactMap(\.string) }
    private var content: [String: JSONValue]? {
        var result: [String: JSONValue] = [:]
        for key in fields {
            let value = values[key] ?? ""
            if value.isEmpty { if required.contains(key) { return nil }; continue }
            switch params["requestedSchema"]["properties"][key]["type"].string {
            case "boolean": result[key] = .bool(value == "true")
            case "integer": guard let number = Int(value) else { return nil }; result[key] = .number(Double(number))
            case "number": guard let number = Double(value), number.isFinite else { return nil }; result[key] = .number(number)
            case "string", nil: result[key] = .string(value)
            default: return nil
            }
        }
        return result
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(params["message"].string ?? "工具需要补充信息。")
            if params["mode"].string == "url" {
                if let raw = params["url"].string, let url = URL(string: raw), ["https", "http"].contains(url.scheme?.lowercased() ?? "") {
                    Link("打开验证页面", destination: url).frame(minHeight: 44)
                }
                Button("已完成") { onSend(.object(["action": .string("accept"), "content": .null])) }.frame(minHeight: 44)
            } else {
                ForEach(fields, id: \.self) { key in
                    let schema = params["requestedSchema"]["properties"][key]
                    let binding = Binding(get: { values[key] ?? "" }, set: { values[key] = $0 })
                    VStack(alignment: .leading, spacing: 6) {
                        Text(schema["title"].string ?? key).font(.subheadline.weight(.medium))
                        if let description = schema["description"].string { Text(description).font(.caption).foregroundStyle(.secondary) }
                        if schema["type"].string == "boolean" {
                            Picker("选择", selection: binding) {
                                Text("请选择").tag(""); Text("是").tag("true"); Text("否").tag("false")
                            }.pickerStyle(.segmented)
                        } else if !schema["enum"].array.isEmpty {
                            Picker("选择", selection: binding) {
                                Text("请选择").tag("")
                                ForEach(schema["enum"].array.compactMap(\.string), id: \.self) { Text($0).tag($0) }
                            }
                        } else {
                            TextField("输入", text: binding, axis: .vertical).textFieldStyle(.roundedBorder)
                        }
                    }.frame(minHeight: 44)
                }
                Button("提交") {
                    if let content { onSend(.object(["action": .string("accept"), "content": .object(content)])) }
                }.buttonStyle(.borderedProminent).disabled(content == nil).frame(minHeight: 44)
            }
            Button("取消", role: .cancel) { onSend(.object(["action": .string("cancel"), "content": .null])) }.frame(minHeight: 44)
        }
    }
}
