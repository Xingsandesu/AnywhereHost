import AppKit
import CoreHID

/// The system owns gesture recognition. This object only emits digitizer reports.
@MainActor final class DesktopNativeTouch {
    static let protocolName = "hid-digitizer-v1"
    private var device: HIDVirtualDevice?
    private var reports = DesktopTouchReport()
    private static var scanTime: UInt16 { UInt16(truncatingIfNeeded: UInt64(ProcessInfo.processInfo.systemUptime * 10_000)) }

    static func open(displayID: CGDirectDisplayID) async throws -> DesktopNativeTouch {
        guard let uuid = CGDisplayCreateUUIDFromDisplayID(displayID)?.takeRetainedValue() else {
            throw HostFailure(message: "无法绑定虚拟触屏的显示器。")
        }
        let identifier = CFUUIDCreateString(nil, uuid) as String
        let properties = HIDVirtualDevice.Properties(
            descriptor: DesktopTouchReport.descriptor, vendorID: 0x4157, productID: 2,
            product: "Anywhere Touch Display", manufacturer: "Anywhere", serialNumber: UUID().uuidString,
            extraProperties: ["DisplayIntegrated": true as NSNumber,
                              "DigitizerCollectionDispatch": true as NSNumber,
                              "CGDirectDisplayID": displayID as NSNumber,
                              // UniversalHID.displayIdentifier uses this wire key.
                              "displayUUID": identifier as NSString])
        guard let device = HIDVirtualDevice(properties: properties) else {
            throw HostFailure(message: "macOS 拒绝创建虚拟触屏。当前 Host 需要有效的 HID 虚拟设备签名权限；辅助功能授权不能替代此权限。")
        }
        await device.activate(delegate: TouchDeviceDelegate())
        let touch = DesktopNativeTouch(); touch.device = device
        // Verify report submission as well as object allocation. This is not proof
        // of AppKit direct-touch delivery; that requires a receiver integration test.
        try await touch.release()
        return touch
    }
    func receive(_ payload: Data) async throws {
        guard let device else { throw HostFailure(message: "虚拟触屏已经断开。") }
        let frame = try JSONDecoder().decode(DesktopTouchFrame.self, from: payload)
        let data = try reports.encode(frame, scanTime: Self.scanTime)
        try await device.dispatchInputReport(data: data, timestamp: .now)
        if frame.contacts.contains(where: { $0.phase == "cancelled" }) {
            // Clear the driver's pressed state after delivering the cancel mask.
            try await device.dispatchInputReport(data: DesktopTouchReport.pack(DesktopTouchReport.liftCancelled(frame.contacts), scanTime: Self.scanTime), timestamp: .now)
        }
    }
    func release() async throws {
        guard let device else { return }
        let lifted = reports.active.values.map { DesktopTouchFrame.Contact(id: $0.id, phase: "ended", x: $0.x, y: $0.y) }
        let data = reports.release(scanTime: Self.scanTime)
        try await device.dispatchInputReport(data: data, timestamp: .now)
        if !lifted.isEmpty {
            try await device.dispatchInputReport(data: DesktopTouchReport.pack(lifted, scanTime: Self.scanTime), timestamp: .now)
        }
    }
    func close() { device = nil }
}

private final class TouchDeviceDelegate: HIDVirtualDeviceDelegate {
    func hidVirtualDevice(_ device: HIDVirtualDevice, receivedSetReportRequestOfType type: HIDReportType, id: HIDReportID?, data: Data) async throws {
        throw HIDDeviceError.unsupported
    }
    func hidVirtualDevice(_ device: HIDVirtualDevice, receivedGetReportRequestOfType type: HIDReportType, id: HIDReportID?, maxSize: Int) async throws -> Data {
        guard type == .feature, id?.rawValue == 2, maxSize >= 2 else { throw HIDDeviceError.unsupported }
        return Data([2, UInt8(DesktopTouchReport.maximumContacts)])
    }
}

/// Local consent is separate from the OS-enforced signature entitlement.
enum DesktopTouchAuthorization {
    static let url = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        .appending(path: "AnywhereHost/native-touch.json")
    static var isEnabled: Bool {
        guard let data = try? Data(contentsOf: url), let value = try? JSONDecoder().decode([String: Bool].self, from: data) else { return false }
        return value["enabled"] == true
    }
    static func save(_ enabled: Bool) throws {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        try JSONEncoder().encode(["enabled": enabled]).write(to: url, options: .atomic)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
    }
}
