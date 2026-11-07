import AppKit

struct HostFailure: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

@main enum TouchProbeMain {
    @MainActor static func main() {
        if ProcessInfo.processInfo.arguments.contains("--receive-touch-frames") {
            NativeTouchStreamProbe.run(); return
        }
        guard ProcessInfo.processInfo.arguments.contains("--inspect-device") else {
            DesktopNativeTouchProbe.run(); return
        }
        NSApplication.shared.setActivationPolicy(.prohibited)
        Task { @MainActor in
            do {
                let device = try await DesktopNativeTouch.open(displayID: CGMainDisplayID())
                fputs("Virtual touchscreen registered for 30 seconds; no contacts will be sent.\n", stderr)
                try await Task.sleep(for: .seconds(30))
                device.close(); exit(0)
            } catch {
                fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1)
            }
        }
        NSApplication.shared.run()
    }
}
