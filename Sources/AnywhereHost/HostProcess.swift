import Foundation

@MainActor
final class HostProcess {
    let stateDirectory = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appending(path: "AnywhereHost")
    private var process: Process?
    func start() throws {
        if process?.isRunning == true { return }
        let binary = Bundle.main.bundleURL.appending(path: "Contents/MacOS/anywhere-host")
        guard FileManager.default.isExecutableFile(atPath: binary.path) else { throw HostFailure(message: "后端未打包。请运行 Scripts/build-app.sh 后打开 dist/AnywhereHost.app。") }
        try FileManager.default.createDirectory(at: stateDirectory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let log = stateDirectory.appending(path: "host.log")
        if !FileManager.default.fileExists(atPath: log.path) { FileManager.default.createFile(atPath: log.path, contents: nil, attributes: [.posixPermissions: 0o600]) }
        let output = try FileHandle(forWritingTo: log)
        try output.truncate(atOffset: 0)
        let p = Process()
        p.executableURL = binary
        p.arguments = ["--state-dir", stateDirectory.path]
        var environment = ProcessInfo.processInfo.environment
        let userHome = FileManager.default.homeDirectoryForCurrentUser
        let home = userHome.path
        environment["HOME"] = home
        environment["CODEX_HOME"] = environment["CODEX_HOME"] ?? userHome.appending(path: ".codex").path
        p.currentDirectoryURL = userHome
        let search = ["\(home)/.cargo/bin", "\(home)/.local/bin", "/opt/homebrew/bin", "/usr/local/bin", environment["PATH"] ?? "/usr/bin:/bin:/usr/sbin:/sbin"]
        environment["PATH"] = search.joined(separator: ":")
        p.environment = environment
        p.standardOutput = output; p.standardError = output; p.standardInput = FileHandle.nullDevice
        defer { try? output.close() }
        try p.run()
        process = p
    }
    func stop() {
        guard let process else { return }
        if process.isRunning { process.terminate(); process.waitUntilExit() }
        self.process = nil
    }
    var isRunning: Bool { process?.isRunning == true }
}
