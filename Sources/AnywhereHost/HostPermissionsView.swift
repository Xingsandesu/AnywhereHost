import AppKit
import SwiftUI

struct HostPermissionsView: View {
    var store: HostStore
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Label("主机文件权限", systemImage: "externaldrive.badge.checkmark").font(.headline)
            Text("1. 在系统设置的“完全磁盘访问权限”中添加并启用 Anywhere Host；macOS 提示重新打开时，请退出并重新启动主机。")
            HStack {
                Button("打开完全磁盘访问权限", systemImage: "gearshape") {
                    Task { await store.perform { try store.permissions.openFullDiskAccess() } }
                }
                Button("在 Finder 中显示应用") { NSWorkspace.shared.activateFileViewerSelecting([Bundle.main.bundleURL]) }
            }
            Text("2. 授权用户主目录，让沙盒内的 Codex 读取登录与历史，并在项目中执行命令、修改文件。完全磁盘访问权限不会代替此项授权。")
            if store.permissions.homeAuthorized {
                Label("主目录已授权", systemImage: "checkmark.circle.fill").foregroundStyle(.green)
            } else {
                Button("授权用户主目录…", systemImage: "folder.badge.plus") {
                    Task { await store.perform { if try await store.permissions.authorizeHome() { await store.start() } } }
                }.buttonStyle(.borderedProminent).disabled(store.busy)
            }
            ForEach(store.permissions.folders, id: \.path) { url in
                Text(verbatim: url.path).font(.caption.monospaced()).textSelection(.enabled)
            }
            if let issue = store.permissions.issue { Text(verbatim: issue).foregroundStyle(.orange) }
            if store.permissions.homeAuthorized {
                if store.client == nil {
                    Text("Homebrew 位于 /opt/homebrew 或 /usr/local 时，也需要授权对应目录。")
                        .font(.caption).foregroundStyle(.secondary)
                    HStack {
                        Button("授权开发工具…") {
                            Task { await store.perform { _ = try await store.permissions.authorizeFolder(suggestedURL: URL(fileURLWithPath: "/opt/homebrew")) } }
                        }
                        Button("授权其他文件夹…") { Task { await store.perform { _ = try await store.permissions.authorizeFolder() } } }
                        Button("启动主机") { Task { await store.start() } }
                    }.disabled(store.busy || store.isStarting)
                } else {
                    Text("需要授权外置磁盘或其他目录时，先结束 Codex 任务，再停止主机。新增授权会在下次启动时传给所有子进程。")
                        .font(.caption).foregroundStyle(.secondary)
                    Button("停止主机") { store.stop() }.disabled(store.busy || store.isStarting)
                }
            }
        }
        .font(.callout)
        .padding(20).frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(nsColor: .controlBackgroundColor), in: .rect(cornerRadius: 14))
    }
}
