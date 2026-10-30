import Foundation
import Testing
@testable import AnywhereHost

@MainActor struct HostPermissionTests {
    private func withDefaults(_ body: (UserDefaults) async throws -> Void) async rethrows {
        let name = "AnywhereHostPermissionTests.\(UUID())"
        let defaults = UserDefaults(suiteName: name)!
        defer { defaults.removePersistentDomain(forName: name) }
        try await body(defaults)
    }
    @Test func skippingAndUncheckedCompletionDoNotPersist() async {
        await withDefaults { defaults in
            let center = HostPermissionCenter(defaults: defaults)
            #expect(center.showOnboarding)
            center.deferOnboarding()
            #expect(!center.showOnboarding)
            #expect(HostPermissionCenter(defaults: defaults).showOnboarding)
            center.completeOnboarding()
            #expect(!center.hasCompletedOnboarding)
        }
    }
    @Test func onlyReadWriteSuccessPasses() throws {
        let cases: [(String, DirectoryPermissionState)] = [
            (#"{"readable":true,"writable":true,"issue":null}"#, .ready),
            (#"{"readable":true,"writable":false,"issue":"permissionDenied"}"#, .readOnly),
            (#"{"readable":false,"writable":false,"issue":"permissionDenied"}"#, .denied),
            (#"{"readable":false,"writable":false,"issue":"missing"}"#, .missing),
            (#"{"readable":false,"writable":false,"issue":"ioError"}"#, .unavailable)
        ]
        for (json, expected) in cases {
            #expect(try JSONDecoder().decode(DirectoryAccessResult.self, from: Data(json.utf8)).state == expected)
        }
    }
    @Test func duplicatePathsKeepRequiredStatus() async {
        await withDefaults { defaults in
            let center = HostPermissionCenter(defaults: defaults)
            let path = FileManager.default.homeDirectoryForCurrentUser.appending(path: "Documents").path
            center.configure(stateDirectory: URL(fileURLWithPath: "/tmp/state"), codexHome: "/tmp/codex", projectRoots: [path, path])
            #expect(center.directories.filter { $0.path == path }.count == 1)
            #expect(center.directories.first { $0.path == path }?.required == true)
            #expect(!center.canFinishChecks)
        }
    }
    @Test func successfulSetupPersistsButRevocationAndNewProjectsNeedChecks() async throws {
        try await withDefaults { defaults in
            let center = HostPermissionCenter(defaults: defaults)
            center.configure(stateDirectory: URL(fileURLWithPath: "/tmp/state"), codexHome: "/tmp/codex", projectRoots: [])
            let fixture = try makeProbe("printf '%s' '{\"readable\":true,\"writable\":true,\"issue\":null}'")
            defer { try? FileManager.default.removeItem(at: fixture.deletingLastPathComponent()) }
            await center.checkAll(using: DirectoryPermissionProbe(executable: fixture), includeOptional: false)
            #expect(center.canFinishChecks)
            #expect(center.passedCount == 2)
            center.completeOnboarding()
            #expect(!HostPermissionCenter(defaults: defaults).showOnboarding)
            center.configure(stateDirectory: URL(fileURLWithPath: "/tmp/state"), codexHome: "/tmp/codex", projectRoots: ["/tmp/new-project"])
            #expect(!center.canFinishChecks)
            await center.checkAll(using: DirectoryPermissionProbe(executable: URL(fileURLWithPath: "/usr/bin/false")))
            #expect(!center.canFinishChecks)
            #expect(!center.isChecking)
            #expect(center.passedCount == 0)
        }
    }
    @Test func hungProbeAndBothCancellationPathsAreBounded() async throws {
        let fixture = try makeProbe("exec /bin/sleep 30")
        defer { try? FileManager.default.removeItem(at: fixture.deletingLastPathComponent()) }
        let probe = DirectoryPermissionProbe(executable: fixture, timeout: .milliseconds(150))
        let clock = ContinuousClock(); let start = clock.now
        #expect(await probe.check("/tmp") == .timedOut)
        #expect(clock.now - start < .seconds(3))
        try await withDefaults { defaults in
            let center = HostPermissionCenter(defaults: defaults)
            center.configure(stateDirectory: URL(fileURLWithPath: "/tmp/state"), codexHome: "/tmp/codex", projectRoots: [])
            for cancelViaButton in [true, false] {
                let task = Task { await center.checkAll(using: DirectoryPermissionProbe(executable: fixture)) }
                try await Task.sleep(for: .milliseconds(150))
                if cancelViaButton { center.cancelChecks() } else { task.cancel() }
                await task.value
                #expect(!center.isChecking)
                #expect(!center.states.values.contains(.checking))
                #expect(!center.canFinishChecks)
            }
        }
    }
    private func makeProbe(_ contents: String) throws -> URL {
        let directory = FileManager.default.temporaryDirectory.appending(path: "host-probe-tests-\(UUID())")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let url = directory.appending(path: "probe")
        try Data("#!/bin/sh\n\(contents)\n".utf8).write(to: url)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: url.path)
        return url
    }
}
