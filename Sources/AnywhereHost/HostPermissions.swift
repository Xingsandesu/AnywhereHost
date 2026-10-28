import AppKit
import Foundation
import Observation

/// Keep the scopes alive before spawning services and for their entire lifetime.
@MainActor @Observable
final class HostPermissions {
    private(set) var homeAuthorized = false
    private(set) var folders: [URL] = []
    private(set) var issue: String?
    private var scopes: [URL] = []
    private let defaults: UserDefaults
    private let bookmarkKey = "hostFolderBookmarks"

    // Foundation's home URL points inside the container when sandboxed.
    let userHome: URL = {
        guard let entry = getpwuid(getuid()) else { return FileManager.default.homeDirectoryForCurrentUser }
        return URL(fileURLWithPath: String(cString: entry.pointee.pw_dir), isDirectory: true)
    }()

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        let saved = defaults.dictionary(forKey: bookmarkKey) as? [String: Data] ?? [:]
        var refreshed = saved
        for (path, data) in saved {
            do {
                var stale = false
                let url = try URL(resolvingBookmarkData: data, options: [.withSecurityScope, .withoutUI], relativeTo: nil, bookmarkDataIsStale: &stale)
                guard url.startAccessingSecurityScopedResource() else { throw HostFailure(message: "文件夹授权已失效，请重新选择：\(path)") }
                scopes.append(url)
                if stale {
                    refreshed.removeValue(forKey: path)
                    refreshed[url.path] = try url.bookmarkData(options: .withSecurityScope, includingResourceValuesForKeys: nil, relativeTo: nil)
                }
                folders.append(url)
            } catch { issue = error.localizedDescription }
        }
        defaults.set(refreshed, forKey: bookmarkKey)
        updateHomeAccess()
    }

    func authorizeHome() async throws -> Bool {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true; panel.canChooseFiles = false
        panel.directoryURL = userHome
        panel.prompt = "授权主目录"
        panel.message = "请选择你的用户主目录。Anywhere Host 将在此使用现有 Codex 登录、历史、项目和开发工具。"
        panel.showsHiddenFiles = true
        guard await panel.begin() == .OK, let url = panel.url else { return false }
        guard url.standardizedFileURL == userHome.standardizedFileURL else {
            throw HostFailure(message: "请选择用户主目录：\(userHome.path)。外置磁盘或其他目录可在主机停止时另行授权。")
        }
        try remember(url)
        return true
    }

    func authorizeFolder(suggestedURL: URL? = nil) async throws -> Bool {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true; panel.canChooseFiles = false
        panel.directoryURL = suggestedURL
        panel.prompt = "授权文件夹"
        panel.message = "选择外置磁盘、项目或工具链目录。启动主机后，Codex 将能读写该目录。"
        guard await panel.begin() == .OK, let url = panel.url else { return false }
        try remember(url)
        return true
    }

    private func remember(_ url: URL) throws {
        guard url.startAccessingSecurityScopedResource() else {
            throw HostFailure(message: "无法取得文件夹授权，请重新选择。")
        }
        do {
            let data = try url.bookmarkData(options: .withSecurityScope, includingResourceValuesForKeys: nil, relativeTo: nil)
            var saved = defaults.dictionary(forKey: bookmarkKey) as? [String: Data] ?? [:]
            saved[url.path] = data
            defaults.set(saved, forKey: bookmarkKey)
            if let index = scopes.firstIndex(where: { $0.path == url.path }) { scopes.remove(at: index).stopAccessingSecurityScopedResource() }
            scopes.append(url)
            folders.removeAll { $0.path == url.path }; folders.append(url)
            issue = nil
            updateHomeAccess()
        } catch { url.stopAccessingSecurityScopedResource(); throw error }
    }

    private func updateHomeAccess() {
        homeAuthorized = folders.contains { $0.standardizedFileURL == userHome.standardizedFileURL }
        folders.sort { $0.path < $1.path }
    }

    func covers(_ url: URL) -> Bool {
        let path = url.resolvingSymlinksInPath().standardizedFileURL.path
        return folders.contains {
            let root = $0.resolvingSymlinksInPath().standardizedFileURL.path
            return path == root || path.hasPrefix(root + "/")
        }
    }

    func openFullDiskAccess() throws {
        let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")!
        guard NSWorkspace.shared.open(url) else { throw HostFailure(message: "请打开系统设置 → 隐私与安全性 → 完全磁盘访问权限。") }
    }
}
