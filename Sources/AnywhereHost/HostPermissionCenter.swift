import AppKit
import Foundation
import Observation

struct PermissionDirectory: Identifiable, Equatable, Sendable {
    let path: String
    let title: String
    let required: Bool
    var id: String { path }
}

enum DirectoryPermissionState: Equatable, Sendable {
    case unchecked, checking, ready, readOnly, denied, missing, unavailable, timedOut
    var title: LocalizedStringResource {
        switch self {
        case .unchecked: "未检查"
        case .checking: "检查中"
        case .ready: "可读写"
        case .readOnly: "只能读取"
        case .denied: "访问被拒绝"
        case .missing: "目录不存在"
        case .unavailable: "无法验证"
        case .timedOut: "检查超时"
        }
    }
    var detail: LocalizedStringResource {
        switch self {
        case .unchecked: "检查会读取目录，并创建后删除临时文件，不读取或修改已有文件。"
        case .checking: "如出现系统授权提示，请允许访问；完成后可重新检查。"
        case .ready: "当前 Host 可以读取和写入此目录。"
        case .readOnly: "无法写入或删除临时文件。请检查目录权限、系统授权和磁盘状态。"
        case .denied: "请在系统设置的“文件与文件夹”或“完全磁盘访问权限”中允许当前应用。"
        case .missing: "请确认目录位置；外置磁盘或网络卷可能尚未连接。"
        case .unavailable: "请确认路径、磁盘状态和 Host 安装是否完整。"
        case .timedOut: "请处理系统授权提示，必要时重新打开应用，再检查一次。"
        }
    }
}

struct DirectoryAccessResult: Decodable, Sendable {
    let readable: Bool
    let writable: Bool
    let issue: String?
    var state: DirectoryPermissionState {
        if readable && writable { return .ready }
        if readable { return .readOnly }
        switch issue {
        case "permissionDenied": return .denied
        case "missing": return .missing
        default: return .unavailable
        }
    }
}

/// An owned, short-lived subprocess keeps TCC prompts and unresponsive volumes off the UI thread.
@MainActor struct DirectoryPermissionProbe {
    var executable = Bundle.main.bundleURL.appending(path: "Contents/MacOS/anywhere-permission-probe")
    var timeout: Duration = .seconds(8)

    func check(_ path: String) async -> DirectoryPermissionState {
        let process = Process()
        let output = Pipe()
        process.executableURL = executable
        process.arguments = ["--check-directory-access", path]
        process.currentDirectoryURL = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        defer {
            if process.isRunning {
                process.terminate()
                if process.isRunning { kill(process.processIdentifier, SIGKILL) }
            }
            try? output.fileHandleForReading.close()
            try? output.fileHandleForWriting.close()
        }
        do {
            try Task.checkCancellation()
            try process.run()
            let clock = ContinuousClock()
            let deadline = clock.now.advanced(by: timeout)
            while process.isRunning {
                try Task.checkCancellation()
                if clock.now >= deadline { return .timedOut }
                try await Task.sleep(for: .milliseconds(100))
            }
            try Task.checkCancellation()
            guard process.terminationStatus == 0 else { return .unavailable }
            try output.fileHandleForWriting.close()
            let data = try output.fileHandleForReading.readToEnd() ?? Data()
            return try JSONDecoder().decode(DirectoryAccessResult.self, from: data).state
        } catch { return .unavailable }
    }
}

@MainActor @Observable final class HostPermissionCenter {
    static let onboardingVersion = 1
    private let defaults: UserDefaults
    var showOnboarding: Bool
    var showPermissions = false
    private(set) var directories: [PermissionDirectory] = []
    private(set) var states: [String: DirectoryPermissionState] = [:]
    private(set) var isChecking = false
    private(set) var lastChecked: Date?
    private(set) var hasRequestedCheck = false
    private(set) var selectedDirectory: String?
    var settingsError: String?
    @ObservationIgnored private var activeProbe: Task<DirectoryPermissionState, Never>?

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        showOnboarding = defaults.integer(forKey: "hostOnboardingVersion") < Self.onboardingVersion
        selectedDirectory = defaults.string(forKey: "hostPermissionDirectory")
    }
    var hasCompletedOnboarding: Bool { defaults.integer(forKey: "hostOnboardingVersion") >= Self.onboardingVersion }
    var passedCount: Int { directories.filter { states[$0.id] == .ready }.count }
    var needsAttention: Bool { directories.contains { $0.required && states[$0.id] != .ready } }
    var canFinishChecks: Bool { lastChecked != nil && !isChecking && !directories.isEmpty && !needsAttention }
    var toolbarSymbol: String { lastChecked == nil ? "checklist" : needsAttention ? "exclamationmark.shield" : "checkmark.shield" }
    func openPermissions() { showOnboarding = false; showPermissions = true }
    func openOnboarding() { showPermissions = false; showOnboarding = true }
    func completeOnboarding() {
        guard canFinishChecks else { return }
        defaults.set(Self.onboardingVersion, forKey: "hostOnboardingVersion")
        showOnboarding = false
    }
    // Skipping does not record completion, so the next launch offers setup again.
    func deferOnboarding() { showOnboarding = false }

    func configure(stateDirectory: URL, codexHome: String?, projectRoots: [String]) {
        let home = FileManager.default.homeDirectoryForCurrentUser
        let actualCodexHome = codexHome ?? ProcessInfo.processInfo.environment["CODEX_HOME"] ?? home.appending(path: ".codex").path
        var targets = [PermissionDirectory(path: stateDirectory.path, title: String(localized: "主机数据"), required: true),
                       PermissionDirectory(path: actualCodexHome, title: "Codex Home", required: true)]
        if let selectedDirectory { targets.append(.init(path: selectedDirectory, title: String(localized: "选择的项目目录"), required: true)) }
        targets += projectRoots.map { .init(path: $0, title: String(localized: "项目目录"), required: true) }
        let folders: [(String, LocalizedStringResource)] = [("Documents", "文稿"), ("Desktop", "桌面"), ("Downloads", "下载")]
        targets += folders.map { .init(path: home.appending(path: $0.0).path, title: String(localized: $0.1), required: false) }
        var seen = Set<String>()
        directories = targets.filter { seen.insert($0.path).inserted }
        states = states.filter { seen.contains($0.key) }
    }
    func checkAll(using probe: DirectoryPermissionProbe = .init(), includeOptional: Bool = true) async {
        guard !isChecking else { return }
        isChecking = true; hasRequestedCheck = true
        defer { isChecking = false }
        let targets = directories.filter { includeOptional || $0.required }
        // Sequential probes avoid stacking system consent prompts.
        for target in targets {
            guard !Task.isCancelled else { return }
            states[target.id] = .checking
            let task = Task { await probe.check(target.path) }
            activeProbe = task
            let result = await withTaskCancellationHandler { await task.value } onCancel: { task.cancel() }
            activeProbe = nil
            if task.isCancelled || Task.isCancelled { states[target.id] = .unchecked; return }
            states[target.id] = result
        }
        if !Task.isCancelled { lastChecked = Date() }
    }
    func cancelChecks() { activeProbe?.cancel() }
    func chooseDirectory() async -> Bool {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false; panel.canChooseDirectories = true; panel.allowsMultipleSelection = false
        panel.message = String(localized: "选择实际使用的项目目录，检查 Host 能否读取并保存文件。")
        guard await panel.begin() == .OK, let url = panel.url else { return false }
        selectedDirectory = url.path
        defaults.set(url.path, forKey: "hostPermissionDirectory")
        return true
    }
    func openSettings(_ destination: PermissionSettingsDestination) {
        settingsError = nil
        guard let url = URL(string: destination.url), NSWorkspace.shared.open(url) else {
            settingsError = String(localized: "无法打开系统设置。请手动前往系统设置 → 隐私与安全性，选择对应权限。")
            return
        }
    }
}

enum PermissionSettingsDestination {
    case files, fullDisk, accessibility, screenRecording, automation, localNetwork
    var url: String {
        let pane: String
        switch self {
        case .files: pane = "Privacy_FilesAndFolders"
        case .fullDisk: pane = "Privacy_AllFiles"
        case .accessibility: pane = "Privacy_Accessibility"
        case .screenRecording: pane = "Privacy_ScreenCapture"
        case .automation: pane = "Privacy_Automation"
        case .localNetwork: pane = "Privacy_LocalNetwork"
        }
        return "x-apple.systempreferences:com.apple.preference.security?\(pane)"
    }
}
