import Foundation

/// Read the packaged version so the dashboard and About window agree.
enum HostVersion {
    static var display: String {
        display(info: Bundle.main.infoDictionary ?? [:])
    }
    static func display(info: [String: Any]) -> String {
        guard let version = info["CFBundleShortVersionString"] as? String,
              let build = info["CFBundleVersion"] as? String else { return "开发构建" }
        return "\(version) (\(build))"
    }
}
