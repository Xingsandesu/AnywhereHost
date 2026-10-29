import AppKit
import SwiftUI

struct HostPermissionsView: View {
    var store: HostStore
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Label("主机文件访问", systemImage: "externaldrive.badge.checkmark").font(.headline)
            Text("直接使用这台 Mac 的项目和开发工具，无需逐个授权文件夹。命令执行权限和审批由 Codex 配置决定。")
            Text("需要访问受 macOS 保护的数据时，可在“完全磁盘访问权限”中启用 Anywhere Host，并按系统提示重新打开应用。")
                .foregroundStyle(.secondary)
            HStack {
                Button("打开完全磁盘访问权限", systemImage: "gearshape") {
                    Task {
                        await store.perform {
                            let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")!
                            guard NSWorkspace.shared.open(url) else {
                                throw HostFailure(message: "请打开系统设置 → 隐私与安全性 → 完全磁盘访问权限。")
                            }
                        }
                    }
                }
                Button("在 Finder 中显示应用") { NSWorkspace.shared.activateFileViewerSelecting([Bundle.main.bundleURL]) }
            }
            HStack {
                if store.client == nil {
                    Button("启动主机") { Task { await store.start() } }
                } else {
                    Button("停止主机") { store.stop() }
                }
                Button("打开主机数据目录") { NSWorkspace.shared.open(store.process.stateDirectory) }
            }.disabled(store.busy || store.isStarting)
        }
        .font(.callout)
        .padding(20).frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(nsColor: .controlBackgroundColor), in: .rect(cornerRadius: 14))
    }
}
