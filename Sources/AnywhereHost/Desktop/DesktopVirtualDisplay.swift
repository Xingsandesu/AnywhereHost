import AppKit
import ObjectiveC

/// Runtime-bound CoreGraphics SPI. No Sidecar service impersonation or entitlement assumptions.
/// The object owns the display; releasing it removes the display and returns its windows to macOS.
@MainActor final class DesktopVirtualDisplay {
    private let display: NSObject
    let id: CGDirectDisplayID

    private static func allocate(_ name: String, initializer: String) throws -> (UnsafeRawPointer, IMP) {
        guard let cls = NSClassFromString(name),
              let alloc = class_getClassMethod(cls, NSSelectorFromString("alloc")),
              let initialize = class_getInstanceMethod(cls, NSSelectorFromString(initializer)) else {
            throw HostFailure(message: "当前 macOS 不提供虚拟显示器接口：\(name)")
        }
        typealias Allocate = @convention(c) (AnyClass, Selector) -> UnsafeRawPointer
        return (unsafeBitCast(method_getImplementation(alloc), to: Allocate.self)(cls, NSSelectorFromString("alloc")), method_getImplementation(initialize))
    }
    private static func object(_ name: String) throws -> NSObject {
        let (pointer, implementation) = try allocate(name, initializer: "init")
        typealias Initialize = @convention(c) (UnsafeRawPointer, Selector) -> Unmanaged<NSObject>?
        guard let value = unsafeBitCast(implementation, to: Initialize.self)(pointer, NSSelectorFromString("init")) else {
            throw HostFailure(message: "无法初始化虚拟显示器接口：\(name)")
        }
        return value.takeRetainedValue()
    }
    init(width: Int, height: Int) throws {
        let descriptor = try Self.object("CGVirtualDisplayDescriptor")
        let values: [String: Any] = [
            "name": "Anywhere Display", "maxPixelsWide": 3840, "maxPixelsHigh": 2880,
            "vendorID": 0x4157, "productID": 1, "serialNum": ProcessInfo.processInfo.processIdentifier,
            "serialNumber": ProcessInfo.processInfo.processIdentifier,
            "sizeInMillimeters": NSValue(size: NSSize(width: Double(width) * 25.4 / 220, height: Double(height) * 25.4 / 220)),
            "redPrimary": NSValue(point: NSPoint(x: 0.64, y: 0.33)),
            "greenPrimary": NSValue(point: NSPoint(x: 0.30, y: 0.60)),
            "bluePrimary": NSValue(point: NSPoint(x: 0.15, y: 0.06)),
            "whitePoint": NSValue(point: NSPoint(x: 0.3127, y: 0.3290)),
            "queue": DispatchQueue.main
        ]
        for (key, value) in values {
            let setter = "set" + key.prefix(1).uppercased() + key.dropFirst() + ":"
            guard descriptor.responds(to: NSSelectorFromString(setter)) else {
                throw HostFailure(message: "虚拟显示器接口发生变化：\(key)")
            }
            descriptor.setValue(value, forKey: key)
        }
        let selector = NSSelectorFromString("initWithDescriptor:")
        let (instance, initializeDisplay) = try Self.allocate("CGVirtualDisplay", initializer: NSStringFromSelector(selector))
        typealias Initialize = @convention(c) (UnsafeRawPointer, Selector, AnyObject) -> Unmanaged<NSObject>?
        guard let initialized = unsafeBitCast(initializeDisplay, to: Initialize.self)(instance, selector, descriptor)?.takeRetainedValue() else {
            throw HostFailure(message: "macOS 拒绝创建扩展显示器。")
        }
        guard initialized.responds(to: NSSelectorFromString("applySettings:")), initialized.responds(to: NSSelectorFromString("displayID")) else {
            throw HostFailure(message: "虚拟显示器接口不可用。")
        }
        display = initialized
        id = (initialized.value(forKey: "displayID") as? NSNumber)?.uint32Value ?? 0
        guard id != 0 else { throw HostFailure(message: "虚拟显示器未返回有效标识。") }
        try resize(width: width, height: height)
    }
    func resize(width: Int, height: Int) throws {
        let modeSelector = NSSelectorFromString("initWithWidth:height:refreshRate:")
        let (mode, initializeMode) = try Self.allocate("CGVirtualDisplayMode", initializer: NSStringFromSelector(modeSelector))
        typealias Mode = @convention(c) (UnsafeRawPointer, Selector, UInt32, UInt32, Double) -> Unmanaged<NSObject>?
        guard let configuredMode = unsafeBitCast(initializeMode, to: Mode.self)(mode, modeSelector, UInt32(width / 2), UInt32(height / 2), 60)?.takeRetainedValue() else {
            throw HostFailure(message: "无法创建虚拟显示模式。")
        }
        let settings = try Self.object("CGVirtualDisplaySettings")
        guard settings.responds(to: NSSelectorFromString("setModes:")), settings.responds(to: NSSelectorFromString("setHiDPI:")) else {
            throw HostFailure(message: "虚拟显示器设置不可用。")
        }
        settings.setValue([configuredMode], forKey: "modes"); settings.setValue(1, forKey: "hiDPI")
        typealias Apply = @convention(c) (AnyObject, Selector, AnyObject) -> Bool
        guard unsafeBitCast(display.method(for: NSSelectorFromString("applySettings:")), to: Apply.self)(display, NSSelectorFromString("applySettings:"), settings) else {
            throw HostFailure(message: "macOS 拒绝虚拟显示器设置。")
        }
    }
}
