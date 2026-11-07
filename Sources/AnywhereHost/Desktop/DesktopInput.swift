import AppKit

@MainActor final class DesktopInput {
    var displayID = CGMainDisplayID()
    private var bounds: CGRect { CGDisplayBounds(displayID) }
    private var pencilInProximity = false
    private var buttons = Set<Int>()
    private var keys = Set<CGKeyCode>()
    private var flags: CGEventFlags = []
    private var point = CGPoint.zero
    private let emit: (CGEvent) -> Void
    init(emit: @escaping (CGEvent) -> Void = { $0.post(tap: .cghidEventTap) }) { self.emit = emit }
    private let source = CGEventSource(stateID: .privateState)

    func receive(_ value: [String: Any]) {
        switch value["event"] as? String {
        case "pointer":
            guard let x = value["x"] as? Double, let y = value["y"] as? Double, x.isFinite, y.isFinite else { return }
            point = CGPoint(x: bounds.minX + min(1, max(0, x)) * max(0, bounds.width - 1), y: bounds.minY + min(1, max(0, y)) * max(0, bounds.height - 1))
            let button = min(2, max(0, value["button"] as? Int ?? 0))
            let phase = value["phase"] as? String ?? "move"
            let type: CGEventType
            if phase == "down" {
                buttons.insert(button); type = button == 0 ? .leftMouseDown : button == 1 ? .rightMouseDown : .otherMouseDown
            } else if phase == "up" {
                buttons.remove(button); type = button == 0 ? .leftMouseUp : button == 1 ? .rightMouseUp : .otherMouseUp
            } else { type = buttons.contains(0) ? .leftMouseDragged : buttons.contains(1) ? .rightMouseDragged : .mouseMoved }
            let isPencil = value["kind"] as? String == "pencil"
            if isPencil && !pencilInProximity { proximity(true, eraser: value["eraser"] as? Bool == true) }
            let event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point, mouseButton: CGMouseButton(rawValue: UInt32(button)) ?? .left)
            event?.setIntegerValueField(.mouseEventClickState, value: Int64(min(3, max(1, value["clicks"] as? Int ?? 1))))
            if let pressure = value["pressure"] as? Double, pressure.isFinite { event?.setDoubleValueField(.mouseEventPressure, value: min(1, max(0, pressure))) }
            if isPencil {
                event?.setIntegerValueField(.mouseEventSubtype, value: 1)
                event?.setIntegerValueField(.tabletEventDeviceID, value: 1)
                event?.setIntegerValueField(.tabletEventPointButtons, value: phase == "up" ? 0 : 1)
                event?.setIntegerValueField(.tabletEventPointX, value: Int64(min(1, max(0, x)) * 65535))
                event?.setIntegerValueField(.tabletEventPointY, value: Int64(min(1, max(0, y)) * 65535))
                for (name, field): (String, CGEventField) in [("pressure", .tabletEventPointPressure), ("tiltX", .tabletEventTiltX), ("tiltY", .tabletEventTiltY)] {
                    if let number = value[name] as? Double, number.isFinite { event?.setDoubleValueField(field, value: min(1, max(name == "pressure" ? 0 : -1, number))) }
                }
            }
            post(event)
            if isPencil && phase == "up" { proximity(false, eraser: value["eraser"] as? Bool == true) }
        case "scroll":
            guard let dx = value["dx"] as? Double, let dy = value["dy"] as? Double, dx.isFinite, dy.isFinite else { return }
            let event = CGEvent(scrollWheelEvent2Source: source, units: .pixel, wheelCount: 2, wheel1: Int32(min(4096, max(-4096, dy))), wheel2: Int32(min(4096, max(-4096, dx))), wheel3: 0)
            event?.location = point
            post(event)
        case "key":
            guard let code = value["code"] as? Int, (0...127).contains(code), let down = value["down"] as? Bool else { return }
            let key = CGKeyCode(code)
            if down { keys.insert(key) } else { keys.remove(key) }
            updateFlags()
            post(CGEvent(keyboardEventSource: source, virtualKey: key, keyDown: down))
        case "text":
            guard let text = value["text"] as? String, text.utf8.count <= 4096 else { return }
            // CGEvent Unicode injection preserves committed IME text without changing the clipboard.
            for character in text {
                if character == "\n" || character == "\r" || character == "\t" {
                    let key: CGKeyCode = character == "\t" ? 48 : 36
                    post(CGEvent(keyboardEventSource: source, virtualKey: key, keyDown: true))
                    post(CGEvent(keyboardEventSource: source, virtualKey: key, keyDown: false))
                    continue
                }
                let units = Array(String(character).utf16)
                for down in [true, false] {
                    let event = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: down)
                    units.withUnsafeBufferPointer { event?.keyboardSetUnicodeString(stringLength: $0.count, unicodeString: $0.baseAddress) }
                    post(event)
                }
            }
        case "release": release()
        case "menu": pointerEdge(top: true)
        case "dock": pointerEdge(top: false)
        default: break
        }
    }
    private func pointerEdge(top: Bool) {
        point = CGPoint(x: bounds.midX, y: top ? bounds.minY : bounds.maxY - 1)
        post(CGEvent(mouseEventSource: source, mouseType: .mouseMoved, mouseCursorPosition: point, mouseButton: .left))
    }
    private func proximity(_ entering: Bool, eraser: Bool) {
        let event = CGEvent(source: source); event?.type = .tabletProximity
        event?.setIntegerValueField(.tabletProximityEventDeviceID, value: 1)
        event?.setIntegerValueField(.tabletProximityEventSystemTabletID, value: 1)
        event?.setIntegerValueField(.tabletProximityEventVendorID, value: 0x4157)
        event?.setIntegerValueField(.tabletProximityEventVendorUniqueID, value: 1)
        event?.setIntegerValueField(.tabletProximityEventPointerType, value: eraser ? 3 : 1)
        event?.setIntegerValueField(.tabletProximityEventEnterProximity, value: entering ? 1 : 0)
        event?.setIntegerValueField(.tabletProximityEventCapabilityMask, value: 0x3f)
        pencilInProximity = entering; post(event)
    }
    private func post(_ event: CGEvent?) { guard let event else { return }; event.flags = flags; emit(event) }
    private func updateFlags() {
        flags = []
        for (codes, flag): ([CGKeyCode], CGEventFlags) in [([55, 54], .maskCommand), ([56, 60], .maskShift), ([58, 61], .maskAlternate), ([59, 62], .maskControl)] {
            if codes.contains(where: keys.contains) { flags.insert(flag) }
        }
    }
    func release() {
        if pencilInProximity { proximity(false, eraser: false) }
        for key in Array(keys) {
            keys.remove(key); updateFlags()
            post(CGEvent(keyboardEventSource: source, virtualKey: key, keyDown: false))
        }
        for button in buttons {
            let type: CGEventType = button == 0 ? .leftMouseUp : button == 1 ? .rightMouseUp : .otherMouseUp
            post(CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point, mouseButton: CGMouseButton(rawValue: UInt32(button)) ?? .left))
        }
        buttons.removeAll()
    }
}
