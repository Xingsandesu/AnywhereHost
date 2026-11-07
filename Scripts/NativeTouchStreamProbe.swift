import AppKit

/// Integration-test receiver. Stdin accepts bounded touch/release JSON lines;
/// every contact is confined to this process's window on its owned display.
/// Stdout records actual AppKit callbacks independently of submitted reports.
@MainActor enum NativeTouchStreamProbe {
    static func run() {
        NSApplication.shared.setActivationPolicy(.regular)
        Task { @MainActor in
            do {
                let display = try DesktopVirtualDisplay(width: 1600, height: 1200)
                defer { withExtendedLifetime(display) {} }
                try await display.waitForHiDPIMode(width: 1600, height: 1200)
                guard let screen = NSScreen.screens.first(where: {
                    ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value == display.id
                }) else { throw HostFailure(message: "验证显示器不可用。") }
                let window = TouchStreamWindow(contentRect: screen.frame, styleMask: .borderless, backing: .buffered, defer: false)
                window.isReleasedWhenClosed = false
                window.title = "Anywhere 原生输入链路验证"
                window.backgroundColor = .systemIndigo
                defer { window.close() }
                let recorder = TouchStreamRecorder(target: nil, action: nil)
                recorder.allowedTouchTypes = .direct
                recorder.target = recorder; recorder.action = #selector(TouchStreamRecorder.changed)
                window.contentView!.addGestureRecognizer(recorder)
                window.makeKeyAndOrderFront(nil)
                NSApplication.shared.activate(ignoringOtherApps: true)
                let device = try await DesktopNativeTouch.open(displayID: display.id)
                defer { device.close() }
                try await Task.sleep(for: .milliseconds(300))
                emit(["event": "ready", "displayID": display.id])
                // A single reader preserves ordering without blocking AppKit's run loop.
                while let line = await Task.detached(operation: { readLine() }).value {
                    guard line.utf8.count <= 16_384,
                          let payload = line.data(using: .utf8),
                          let object = try JSONSerialization.jsonObject(with: payload) as? [String: Any] else {
                        throw HostFailure(message: "验证输入无效。")
                    }
                    guard window.isKeyWindow, NSApplication.shared.isActive else {
                        try await device.release()
                        throw HostFailure(message: "验证窗口失去焦点，已停止接收输入。")
                    }
                    switch object["event"] as? String {
                    case "touch": try await device.receive(payload)
                    case "release": try await device.release()
                    default: throw HostFailure(message: "验证仅接受 touch/release。")
                    }
                    emit(["event": "submitted", "sequence": object["sequence"] ?? NSNull(), "input": object["event"] ?? NSNull()])
                }
                try await device.release()
                try await Task.sleep(for: .milliseconds(100))
                emit(["event": "closed"])
                exit(0)
            } catch { emit(["event": "error", "message": error.localizedDescription]); exit(1) }
        }
        NSApplication.shared.run()
    }

    static func emit(_ object: [String: Any]) {
        var value = object
        value["time"] = ProcessInfo.processInfo.systemUptime
        guard let data = try? JSONSerialization.data(withJSONObject: value, options: .sortedKeys) else { return }
        FileHandle.standardOutput.write(data + Data([10]))
    }
}

private final class TouchStreamWindow: NSWindow {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { true }
}

@MainActor private final class TouchStreamRecorder: NSGestureRecognizer {
    private var identities: [AnyHashable: Int] = [:]
    private var point = NSPoint.zero
    @objc func changed() {}
    override func location(in target: NSView?) -> NSPoint { target?.convert(point, from: nil) ?? point }
    private func record(_ event: NSEvent) {
        let contacts: [[String: Any]] = event.touches(matching: .any, in: view).filter { $0.type == .direct }.compactMap { touch in
            guard let key = touch.identity as? AnyHashable else { return nil }
            if identities[key] == nil { identities[key] = identities.count + 1 }
            point = touch.location(in: nil)
            let phase: String
            switch touch.phase {
            case .began: phase = "began"
            case .moved: phase = "moved"
            case .stationary: phase = "stationary"
            case .ended: phase = "ended"
            case .cancelled: phase = "cancelled"
            default: phase = "unknown"
            }
            let location = touch.location(in: view)
            return ["id": identities[key]!, "phase": phase, "x": location.x, "y": location.y]
        }
        NativeTouchStreamProbe.emit(["event": "observed", "contacts": contacts])
    }
    override func touchesBegan(with event: NSEvent) { record(event); state = .began }
    override func touchesMoved(with event: NSEvent) { record(event); state = .changed }
    override func touchesEnded(with event: NSEvent) {
        record(event)
        state = event.touches(matching: .touching, in: view).isEmpty ? .ended : .changed
    }
    override func touchesCancelled(with event: NSEvent) { record(event); state = .cancelled }
}
