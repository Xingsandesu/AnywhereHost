import Foundation
import Security

@MainActor
final class HostProcess {
    let stateDirectory: URL
    init(stateDirectory: URL? = nil) {
        self.stateDirectory = stateDirectory ?? FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appending(path: "AnywhereHost")
    }
    private var process: Process?
    func start(permissions: HostPermissions) throws {
        guard permissions.homeAuthorized else { throw HostFailure(message: "请先授权用户主目录。") }
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
        let home = permissions.userHome.path
        // This hint is emitted only by a signed sandboxed parent. The backend also
        // checks its own signature; manually launching an inherited helper is not enough.
        var ownCode: SecCode?
        var signingInfo: CFDictionary?
        var staticCode: SecStaticCode?
        if SecCodeCopySelf([], &ownCode) == errSecSuccess, let ownCode,
           SecCodeCopyStaticCode(ownCode, [], &staticCode) == errSecSuccess, let staticCode,
           SecCodeCopySigningInformation(staticCode, SecCSFlags(rawValue: kSecCSSigningInformation), &signingInfo) == errSecSuccess,
           let info = signingInfo as? [String: Any],
           let entitlements = info[kSecCodeInfoEntitlementsDict as String] as? [String: Any],
           entitlements["com.apple.security.app-sandbox"] as? Bool == true,
           entitlements["com.apple.security.inherit"] as? Bool != true {
            environment["ANYWHERE_HOST_PARENT_SANDBOXED"] = "1"
        } else {
            environment.removeValue(forKey: "ANYWHERE_HOST_PARENT_SANDBOXED")
        }
        environment["HOME"] = home
        environment["CODEX_HOME"] = environment["CODEX_HOME"] ?? permissions.userHome.appending(path: ".codex").path
        p.currentDirectoryURL = permissions.userHome
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
