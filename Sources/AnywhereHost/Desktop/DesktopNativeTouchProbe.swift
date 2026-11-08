import AppKit

/// Only dispatches digitizer reports, into a window owned by this verifier.
@MainActor enum DesktopNativeTouchProbe {
    static func verify(displayID requestedDisplay: CGDirectDisplayID? = nil, gestures: Bool = false, adaptiveControls: Bool = false) async throws {
        // Use an owned display by default so another app's floating windows and
        // system overlays cannot intercept the verification contacts.
        let ownedDisplay = try requestedDisplay == nil ? DesktopVirtualDisplay(width: 1600, height: 1200) : nil
        defer { withExtendedLifetime(ownedDisplay) {} }
        try await ownedDisplay?.waitForHiDPIMode(width: 1600, height: 1200)
        let displayID = requestedDisplay ?? ownedDisplay!.id
        guard let screen = NSScreen.screens.first(where: { ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value == displayID }) else {
            throw HostFailure(message: "触控验证显示器尚未就绪。")
        }
        print("DISPLAY before HID touchCapabilities=\(screen.touchCapabilities.rawValue) EDR=\(screen.maximumPotentialExtendedDynamicRangeColorComponentValue)")
        let device = try await DesktopNativeTouch.open(displayID: displayID)
        defer { device.close() }
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 560, height: 360), styleMask: [.titled], backing: .buffered, defer: false)
        window.title = "Anywhere 原生触控验证"
        window.isReleasedWhenClosed = false
        window.setFrameOrigin(NSPoint(x: screen.visibleFrame.midX - 280, y: screen.visibleFrame.midY - 180))
        let driver = TouchProbeDriver(device: device, window: window, displayID: displayID)
        defer { window.close() }
        window.makeKeyAndOrderFront(nil)
        NSApplication.shared.activate(ignoringOtherApps: true)
        for _ in 0..<150 {
            if window.isKeyWindow, NSApplication.shared.isActive { break }
            try await Task.sleep(for: .milliseconds(100))
        }
        try await Task.sleep(for: .milliseconds(300))
        let activeScreen = NSScreen.screens.first { ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value == displayID }
        print("DISPLAY after HID touchCapabilities=\(activeScreen?.touchCapabilities.rawValue ?? 0)")
        if adaptiveControls {
            try await verifyAdaptiveControls(driver)
            return
        }
        try await verifyContacts(driver, count: 2)
        try await verifyCancellation(driver)
        if gestures {
            try await verifyContacts(driver, count: 10)
            try await verifyControls(driver)
        }
    }

    private static func verifyContacts(_ driver: TouchProbeDriver, count: Int) async throws {
        let view = NSView(frame: NSRect(x: 0, y: 0, width: 560, height: 360))
        let recorder = DirectTouchRecorder(target: nil, action: nil)
        recorder.allowedTouchTypes = .direct
        recorder.target = recorder; recorder.action = #selector(DirectTouchRecorder.changed)
        view.addGestureRecognizer(recorder)
        try await driver.install(view)
        let points = (0..<count).map { NSPoint(x: 80 + ($0 % 5) * 85, y: 120 + ($0 / 5) * 100) }
        try await driver.send(points, phase: "began")
        try await driver.send(points.enumerated().map { NSPoint(x: $0.element.x + CGFloat($0.offset + 1) * 3, y: $0.element.y + 18) }, phase: "moved")
        try await driver.send(points, phase: "ended")
        let complete = recorder.phases.values.filter { $0.contains([.began, .moved, .ended]) }.count
        let moved = recorder.positions.values.filter { points in
            guard let first = points.first else { return false }
            return points.contains { hypot($0.x - first.x, $0.y - first.y) > 4 }
        }.count
        guard complete == count, recorder.maximumContacts == count, moved == count else {
            throw HostFailure(message: "AppKit 多触点验证未通过：预期 \(count)，完整生命周期 \(complete)，同时触点 \(recorder.maximumContacts)，移动触点 \(moved)。")
        }
        print("PASS native \(count) independent contacts and lifecycle")
    }

    private static func verifyCancellation(_ driver: TouchProbeDriver) async throws {
        let view = NSView(frame: NSRect(x: 0, y: 0, width: 560, height: 360))
        let recorder = DirectTouchRecorder(target: nil, action: nil)
        recorder.allowedTouchTypes = .direct
        recorder.target = recorder; recorder.action = #selector(DirectTouchRecorder.changed)
        view.addGestureRecognizer(recorder)
        try await driver.install(view)
        let points = [NSPoint(x: 180, y: 160), NSPoint(x: 350, y: 160)]
        try await driver.send(points, phase: "began")
        // Start a recognized gesture before cancelling. macOS may defer a
        // stationary two-finger contact while disambiguating system gestures.
        try await driver.send(points.map { NSPoint(x: $0.x, y: $0.y + 24) }, phase: "moved")
        try await driver.device.release()
        try await Task.sleep(for: .milliseconds(100))
        guard recorder.phases.values.filter({ $0.contains([.began, .cancelled]) }).count == 2 else {
            throw HostFailure(message: "AppKit 未收到两个直接触点的取消（阶段 \(recorder.phases.values.map(\.rawValue))），不能启用原生触控。")
        }
        print("PASS native cancellation of two contacts")
        // Reusing IDs after cancellation must start a clean native gesture.
        try await verifyContacts(driver, count: 2)
    }

    private static func verifyControls(_ driver: TouchProbeDriver) async throws {
        let observer = TouchControlObserver()
        let button = NSButton(title: "原生按钮", target: observer, action: #selector(TouchControlObserver.clicked))
        button.bezelStyle = .rounded
        let buttonView = NSView(frame: NSRect(x: 0, y: 0, width: 560, height: 360))
        button.frame = NSRect(x: 180, y: 120, width: 200, height: 100); buttonView.addSubview(button)
        try await driver.install(buttonView)
        try await driver.send([NSPoint(x: 280, y: 170)], phase: "began")
        try await driver.send([NSPoint(x: 280, y: 170)], phase: "ended")
        guard observer.clicks == 1 else { throw HostFailure(message: "原生触摸未触发 AppKit 按钮。") }
        print("PASS native NSButton action")

        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 560, height: 360))
        scroll.hasVerticalScroller = true; scroll.isTouchScrollingEnabled = true
        scroll.documentView = NSView(frame: NSRect(x: 0, y: 0, width: 540, height: 2200))
        scroll.contentView.scroll(to: NSPoint(x: 0, y: 800)); scroll.reflectScrolledClipView(scroll.contentView)
        try await driver.install(scroll)
        let origin = scroll.contentView.bounds.origin.y
        try await driver.path(steps: 16) { t in [NSPoint(x: 280, y: 100 + t * 160)] }
        guard abs(scroll.contentView.bounds.origin.y - origin) > 40 else { throw HostFailure(message: "原生触摸未滚动 NSScrollView。") }
        print("PASS native NSScrollView scrolling")

        let magnifyView = NSView(frame: NSRect(x: 0, y: 0, width: 560, height: 360))
        let magnify = NSMagnificationGestureRecognizer(target: observer, action: #selector(TouchControlObserver.magnified(_:)))
        magnify.allowedTouchTypes = .direct; magnifyView.addGestureRecognizer(magnify)
        try await driver.install(magnifyView)
        try await driver.path(steps: 16) { t in [NSPoint(x: 210 - t * 80, y: 180), NSPoint(x: 350 + t * 80, y: 180)] }
        guard observer.magnification > 0.1 else { throw HostFailure(message: "原生双指未触发 NSMagnificationGestureRecognizer。") }
        print("PASS native magnification \(observer.magnification)")

        let rotateView = NSView(frame: NSRect(x: 0, y: 0, width: 560, height: 360))
        let rotate = NSRotationGestureRecognizer(target: observer, action: #selector(TouchControlObserver.rotated(_:)))
        rotate.allowedTouchTypes = .direct; rotateView.addGestureRecognizer(rotate)
        try await driver.install(rotateView)
        try await driver.path(steps: 20) { t in
            let angle = t * .pi / 3
            return [NSPoint(x: 280 - cos(angle) * 90, y: 180 - sin(angle) * 90), NSPoint(x: 280 + cos(angle) * 90, y: 180 + sin(angle) * 90)]
        }
        guard observer.rotation > 15 else { throw HostFailure(message: "原生双指未触发 NSRotationGestureRecognizer。") }
        print("PASS native rotation \(observer.rotation) degrees")
    }

    // Stock controls receive HID contacts; no resizing, swizzling, synthesized
    // mouse events, or private setters are used to manufacture an expansion.
    private static func verifyAdaptiveControls(_ driver: TouchProbeDriver) async throws {
        let observer = TouchControlObserver()
        let root = NSView(frame: NSRect(x: 0, y: 0, width: 560, height: 360))
        let toggle = NSSwitch(frame: NSRect(x: 90, y: 160, width: 38, height: 24))
        toggle.target = observer; toggle.action = #selector(TouchControlObserver.clicked)
        let slider = NSSlider(value: 0.5, minValue: 0, maxValue: 1, target: observer, action: #selector(TouchControlObserver.clicked))
        slider.frame = NSRect(x: 180, y: 158, width: 200, height: 28)
        let stepper = NSStepper(frame: NSRect(x: 430, y: 155, width: 19, height: 32))
        stepper.minValue = 0; stepper.maxValue = 10; stepper.doubleValue = 5
        stepper.target = observer; stepper.action = #selector(TouchControlObserver.clicked)
        [toggle, slider, stepper].forEach { root.addSubview($0) }
        try await driver.install(root)
        for (name, control) in [("NSSwitch", toggle as NSControl), ("NSSlider", slider as NSControl), ("NSStepper", stepper as NSControl)] {
            let before = control.frame
            let windowsBefore = Set(NSApplication.shared.windows.filter(\.isVisible).map(\.windowNumber))
            let clicksBefore = observer.clicks
            let valueBefore = control === toggle ? Double(toggle.state.rawValue) : control.doubleValue
            let point = NSPoint(x: before.midX, y: control === stepper ? before.maxY - 6 : before.midY)
            try await driver.send([point], phase: "began", delay: .milliseconds(250))
            let during = control.frame
            let newWindows = NSApplication.shared.windows.filter { $0.isVisible && !windowsBefore.contains($0.windowNumber) }
            print("OBSERVE \(name) frameBefore=\(before) frameDuring=\(during) newVisibleWindows=\(newWindows.map { String(describing: type(of: $0)) })")
            let end = control === slider ? NSPoint(x: point.x + 50, y: point.y) : point
            if control === slider { try await driver.send([end], phase: "moved") }
            try await driver.send([end], phase: "ended")
            let valueAfter = control === toggle ? Double(toggle.state.rawValue) : control.doubleValue
            guard observer.clicks > clicksBefore, valueAfter != valueBefore else {
                throw HostFailure(message: "原生触摸未改变 \(name) 的值或触发 action。")
            }
            print("PASS native \(name) value \(valueBefore) -> \(valueAfter); expansion is observation only")
        }
    }

    static func run() {
        NSApplication.shared.setActivationPolicy(.regular)
        Task { @MainActor in
            do {
                let extended = ProcessInfo.processInfo.arguments.contains("--extended")
                let gestures = ProcessInfo.processInfo.arguments.contains("--gestures")
                let adaptiveControls = ProcessInfo.processInfo.arguments.contains("--adaptive-controls")
                print("RUNTIME \(ProcessInfo.processInfo.operatingSystemVersionString)")
                if extended {
                    let display = try DesktopVirtualDisplay(width: 2560, height: 1600)
                    try await display.waitForHiDPIMode(width: 2560, height: 1600)
                    try await verify(displayID: display.id, gestures: gestures, adaptiveControls: adaptiveControls)
                    try display.resize(width: 1600, height: 2400)
                    try await display.waitForHiDPIMode(width: 1600, height: 2400)
                    try await verify(displayID: display.id, gestures: gestures, adaptiveControls: adaptiveControls)
                    withExtendedLifetime(display) {}
                    print("PASS extended display landscape and portrait")
                } else { try await verify(gestures: gestures, adaptiveControls: adaptiveControls) }
                print(adaptiveControls ? "PASS AppKit native control actions; visual expansion is not asserted" : "PASS AppKit received multiple direct contacts with begin/move/end/cancel")
                exit(0)
            } catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
        }
        NSApplication.shared.run()
    }
}

@MainActor private final class TouchProbeDriver {
    let device: DesktopNativeTouch
    let window: NSWindow
    let displayID: CGDirectDisplayID
    private var sequence = 0
    init(device: DesktopNativeTouch, window: NSWindow, displayID: CGDirectDisplayID) {
        self.device = device; self.window = window; self.displayID = displayID
    }
    func install(_ view: NSView) async throws {
        // Keep a stable window container while installing individual test views.
        guard let root = window.contentView else { throw HostFailure(message: "触控验证窗口没有内容视图。") }
        root.subviews.forEach { $0.removeFromSuperview() }
        view.frame = root.bounds; view.autoresizingMask = [.width, .height]
        root.addSubview(view)
        window.displayIfNeeded()
        try await Task.sleep(for: .milliseconds(150))
    }
    func send(_ points: [NSPoint], phase: String, delay: Duration = .milliseconds(60)) async throws {
        guard window.isKeyWindow, NSApplication.shared.isActive else {
            try? await device.release()
            throw HostFailure(message: "触控验证窗口失去焦点，已停止并释放触点。")
        }
        let bounds = CGDisplayBounds(displayID)
        let contacts: [[String: Any]] = points.enumerated().map { index, point in
            let screenPoint = window.convertPoint(toScreen: window.contentView!.convert(point, to: nil))
            return ["id": index + 1, "phase": phase,
                    "x": (screenPoint.x - bounds.minX) / bounds.width,
                    "y": (CGDisplayBounds(CGMainDisplayID()).height - screenPoint.y - bounds.minY) / bounds.height]
        }
        sequence += 1
        try await device.receive(JSONSerialization.data(withJSONObject: ["sequence": sequence, "contacts": contacts]))
        try await Task.sleep(for: delay)
    }
    func path(steps: Int, points: (Double) -> [NSPoint]) async throws {
        try await send(points(0), phase: "began")
        for step in 1...steps { try await send(points(Double(step) / Double(steps)), phase: "moved", delay: .milliseconds(16)) }
        try await send(points(1), phase: "ended", delay: .milliseconds(150))
    }
}

@MainActor private final class DirectTouchRecorder: NSGestureRecognizer {
    var phases: [AnyHashable: NSTouch.Phase] = [:]
    var positions: [AnyHashable: [NSPoint]] = [:]
    var maximumContacts = 0
    private var pointInWindow = NSPoint.zero
    @objc func changed() {}
    override func location(in target: NSView?) -> NSPoint { target?.convert(pointInWindow, from: nil) ?? pointInWindow }
    private func record(_ event: NSEvent) {
        let contacts = event.touches(matching: .any, in: view).filter { $0.type == .direct }
        maximumContacts = max(maximumContacts, contacts.filter { $0.phase.intersection(.touching).isEmpty == false }.count)
        for touch in contacts {
            guard let identity = touch.identity as? AnyHashable else { continue }
            phases[identity, default: []].formUnion(touch.phase)
            positions[identity, default: []].append(touch.location(in: view))
            pointInWindow = touch.location(in: nil)
        }
    }
    override func touchesBegan(with event: NSEvent) { record(event); state = .began }
    override func touchesMoved(with event: NSEvent) { record(event); state = .changed }
    override func touchesEnded(with event: NSEvent) { record(event); state = .ended }
    override func touchesCancelled(with event: NSEvent) { record(event); state = .cancelled }
}

@MainActor private final class TouchControlObserver: NSObject {
    var clicks = 0
    var magnification = 0.0
    var rotation = 0.0
    @objc func clicked() { clicks += 1 }
    @objc func magnified(_ gesture: NSMagnificationGestureRecognizer) { magnification = max(magnification, abs(gesture.magnification)) }
    @objc func rotated(_ gesture: NSRotationGestureRecognizer) { rotation = max(rotation, abs(gesture.rotationInDegrees)) }
}
