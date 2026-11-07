import AppKit
import ScreenCaptureKit

@MainActor final class NativeDesktopSession: NSObject, SCStreamDelegate {
    static var current: NativeDesktopSession?
    private var stream: SCStream?
    private var encoder: DesktopEncoder?
    private var virtualDisplay: DesktopVirtualDisplay?
    private let input = DesktopInput()
    private var nativeTouch: DesktopNativeTouch?
    private var touchAuthorizationMonitor: Task<Void, Never>?
    private var started = false
    private var videoGeneration = 0
    private var captureSize = CGSize.zero
    private var termination: DispatchSourceSignal?

    static func run() {
        signal(SIGPIPE, SIG_IGN)
        NSApplication.shared.setActivationPolicy(.prohibited)
        let session = NativeDesktopSession(); current = session
        signal(SIGTERM, SIG_IGN)
        let source = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
        source.setEventHandler { session.finish() }; source.resume(); session.termination = source
        DispatchQueue.global(qos: .userInteractive).async {
            do {
                while true {
                    let (type, payload) = try DesktopWire.receive(.standardInput, maximumLength: 16_385)
                    guard type == 3, payload.count <= 16_384 else { throw CocoaError(.fileReadCorruptFile) }
                    let object = try JSONSerialization.jsonObject(with: payload)
                    guard let value = object as? [String: Any] else { throw CocoaError(.fileReadCorruptFile) }
                    // Decode into Sendable data again on the actor, never transfer an untyped dictionary.
                    _ = value
                    let consumed = DispatchSemaphore(value: 0)
                    Task { @MainActor in await session.receive(payload); consumed.signal() }
                    consumed.wait()
                }
            } catch { Task { @MainActor in session.finish() } }
        }
        NSApplication.shared.run()
    }
    private func receive(_ payload: Data) async {
        do {
            guard let value = try JSONSerialization.jsonObject(with: payload) as? [String: Any] else { return }
            guard started || value["event"] as? String == "start" else { throw HostFailure(message: "请先初始化远程显示会话。") }
            if value["event"] as? String == "start" {
                guard !started else { return }; started = true
                try await start(value)
            } else if value["event"] as? String == "ack" {
                if value["generation"] as? Int == videoGeneration { encoder?.acknowledge() }
            }
            else if value["event"] as? String == "resize" { try await configureCapture(value) }
            else if value["event"] as? String == "touch" {
                guard let nativeTouch else { throw HostFailure(message: "此会话没有启用原生触控。") }
                try await nativeTouch.receive(payload)
            }
            else if started {
                if value["event"] as? String == "release" { try await nativeTouch?.release() }
                input.receive(value)
            }
        } catch {
            try? DesktopOutput.shared.write(4, Data(error.localizedDescription.utf8))
            finish()
        }
    }
    private func start(_ options: [String: Any]) async throws {
        guard CGPreflightScreenCaptureAccess() else { throw HostFailure(message: "请在 Mac 系统设置 → 隐私与安全性 → 屏幕与系统音频录制中允许 Anywhere Host，然后重启 Host。") }
        guard AXIsProcessTrusted() else { throw HostFailure(message: "请在 Mac 系统设置 → 隐私与安全性 → 辅助功能中允许 Anywhere Host。") }
        if options["mode"] as? String == "extend" {
            let size = Self.requestedSize(options, extended: true)
            virtualDisplay = try DesktopVirtualDisplay(width: Int(size.width), height: Int(size.height))
        }
        if options["touchProtocol"] as? String == DesktopNativeTouch.protocolName {
            guard DesktopTouchAuthorization.isEnabled else {
                throw HostFailure(message: "请在 Mac 的 Anywhere Host → 远程桌面中检测并启用虚拟触屏。")
            }
            nativeTouch = try await DesktopNativeTouch.open(displayID: virtualDisplay?.id ?? CGMainDisplayID())
            try DesktopOutput.shared.write(5, JSONSerialization.data(withJSONObject: ["touchProtocol": DesktopNativeTouch.protocolName]))
            touchAuthorizationMonitor = Task { [weak self] in
                while !Task.isCancelled {
                    do { try await Task.sleep(for: .milliseconds(500)) } catch { return }
                    guard let self else { return }
                    if !DesktopTouchAuthorization.isEnabled {
                        try? DesktopOutput.shared.write(4, Data("Mac 已撤销原生触控授权。".utf8))
                        finish(); return
                    }
                }
            }
        }
        try await configureCapture(options)
    }
    static func requestedSize(_ options: [String: Any], extended: Bool) -> CGSize {
        var width = Double(max(2, options["width"] as? Int ?? 2560))
        var height = Double(max(2, options["height"] as? Int ?? 1600))
        if extended {
            // Keep the client's aspect ratio while meeting macOS's HiDPI mode minimum.
            let grow = max(1, 1600 / width, 1200 / height)
            width *= grow; height *= grow
        }
        let shrink = min(1, 3840 / width, 2880 / height)
        width *= shrink; height *= shrink
        return CGSize(width: max(2, Int(width) / 2 * 2), height: max(2, Int(height) / 2 * 2))
    }
    private func configureCapture(_ options: [String: Any]) async throws {
        let size = Self.requestedSize(options, extended: virtualDisplay != nil)
        guard size != captureSize else { return }
        input.release()
        try await nativeTouch?.release()
        if let previous = stream {
            stream = nil
            try await previous.stopCapture()
            if let encoder { try previous.removeStreamOutput(encoder, type: .screen); encoder.stop() }
            encoder = nil
        }
        if let virtualDisplay {
            try virtualDisplay.resize(width: Int(size.width), height: Int(size.height))
            try await virtualDisplay.waitForHiDPIMode(width: Int(size.width), height: Int(size.height))
        }
        let displayID = virtualDisplay?.id ?? CGMainDisplayID()
        var content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
        for _ in 0..<20 where !content.displays.contains(where: { $0.displayID == displayID }) {
            try await Task.sleep(for: .milliseconds(100))
            content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
        }
        guard let display = content.displays.first(where: { $0.displayID == displayID }) else { throw HostFailure(message: "无法找到要共享的显示器。") }
        input.displayID = displayID
        let bounds = CGDisplayBounds(displayID)
        let ratio = min(size.width / bounds.width, size.height / bounds.height)
        let width = max(2, Int(bounds.width * ratio) / 2 * 2)
        let height = max(2, Int(bounds.height * ratio) / 2 * 2)
        videoGeneration += 1
        let encoder = try DesktopEncoder(width: width, height: height, generation: videoGeneration); self.encoder = encoder
        let configuration = SCStreamConfiguration()
        configuration.width = width; configuration.height = height
        configuration.minimumFrameInterval = CMTime(value: 1, timescale: 60)
        configuration.queueDepth = 3; configuration.showsCursor = true
        configuration.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
        configuration.colorSpaceName = CGColorSpace.sRGB
        configuration.colorMatrix = CGDisplayStream.yCbCrMatrix_ITU_R_709_2
        let stream = SCStream(filter: SCContentFilter(display: display, excludingWindows: []), configuration: configuration, delegate: self)
        self.stream = stream
        try stream.addStreamOutput(encoder, type: .screen, sampleHandlerQueue: encoder.queue)
        try await stream.startCapture()
        captureSize = size
    }
    nonisolated func stream(_ stream: SCStream, didStopWithError error: Error) {
        let stoppedStream = ObjectIdentifier(stream)
        Task { @MainActor in
            guard let active = self.stream, ObjectIdentifier(active) == stoppedStream else { return }
            try? DesktopOutput.shared.write(4, Data(error.localizedDescription.utf8))
            finish()
        }
    }
    func finish() {
        touchAuthorizationMonitor?.cancel()
        nativeTouch?.close(); nativeTouch = nil
        input.release()
        virtualDisplay = nil
        // Process exit also tears down capture/codec if stdout is blocked.
        exit(0)
    }
}

@main enum AnywhereHostEntry {
    @MainActor static func main() {
        if ProcessInfo.processInfo.arguments.contains("--desktop-session") { NativeDesktopSession.run() }
        else if ProcessInfo.processInfo.arguments.contains("--verify-native-touch") { DesktopNativeTouchProbe.run() }
        else { AnywhereHostApp.main() }
    }
}
