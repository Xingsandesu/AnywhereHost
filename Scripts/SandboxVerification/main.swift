import AppKit
import Foundation

@MainActor final class VerificationDelegate: NSObject, NSApplicationDelegate {
    let permissions = HostPermissions()
    let directory = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appending(path: "SandboxVerification")
    var host: HostProcess?
    private func verifyBoundaries() throws {
        let outside = URL(fileURLWithPath: "/Users/Shared/AnywhereHost-SandboxVerification")
        guard FileManager.default.fileExists(atPath: outside.path) else {
            throw HostFailure(message: "Create the /Users/Shared/AnywhereHost-SandboxVerification fixture before running")
        }
        var checks: [String: Bool] = [:]
        checks["restoredHomeScope"] = permissions.homeAuthorized
        checks["parentCannotReadOutsideGrant"] = (try? Data(contentsOf: outside.appending(path: "read-probe"))) == nil
        do {
            try Data("must be denied".utf8).write(to: outside.appending(path: "parent-write-probe"))
            checks["parentCannotWriteOutsideGrant"] = false
        } catch { checks["parentCannotWriteOutsideGrant"] = true }
        let child = Process()
        child.executableURL = URL(fileURLWithPath: "/bin/cat")
        child.arguments = [outside.appending(path: "read-probe").path]
        child.standardOutput = FileHandle.nullDevice; child.standardError = FileHandle.nullDevice
        try child.run(); child.waitUntilExit()
        checks["childCannotReadOutsideGrant"] = child.terminationStatus != 0
        try JSONSerialization.data(withJSONObject: checks, options: [.sortedKeys]).write(to: directory.appending(path: "verification-checks.json"))
        guard checks.values.allSatisfy({ $0 }) else { throw HostFailure(message: "Sandbox boundary verification failed") }
    }
    func applicationDidFinishLaunching(_ notification: Notification) {
        Task {
            do {
                if !permissions.homeAuthorized {
                    guard try await permissions.authorizeHome() else { throw HostFailure(message: "Folder authorization cancelled") }
                }
                try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
                let config = directory.appending(path: "config.json")
                if !FileManager.default.fileExists(atPath: config.path) {
                    let settings: [String: Any] = ["listen": "127.0.0.1:19443", "codexHome": permissions.userHome.appending(path: ".codex").path]
                    try JSONSerialization.data(withJSONObject: settings).write(to: config)
                }
                try verifyBoundaries()
                host = HostProcess(stateDirectory: directory)
                try host!.start(permissions: permissions)
                try Data("started\n".utf8).write(to: directory.appending(path: "verification-status"))
            } catch {
                try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
                try? Data(error.localizedDescription.utf8).write(to: directory.appending(path: "verification-status"))
            }
        }
    }
    func applicationWillTerminate(_ notification: Notification) { host?.stop() }
}
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let delegate = VerificationDelegate()
app.delegate = delegate
app.activate(ignoringOtherApps: true)
app.run()
