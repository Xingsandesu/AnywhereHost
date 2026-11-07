import Foundation

/// A complete parallel touchscreen report, never a mouse/scroll translation.
struct DesktopTouchFrame: Decodable {
    struct Contact: Decodable {
        let id: Int
        let phase: String
        let x: Double
        let y: Double
        var isActive: Bool { phase != "ended" && phase != "cancelled" }
    }
    let sequence: UInt64
    let contacts: [Contact]
}

struct DesktopTouchReport {
    static let maximumContacts = 10
    static let byteCount = 1 + maximumContacts * 6 + 3
    private(set) var active: [Int: DesktopTouchFrame.Contact] = [:]
    private var sequence: UInt64?

    // USB HID Digitizers / Touch Screen: ten Finger collections, Scan Time,
    // Contact Count, and a Contact Count Maximum feature report (ID 2).
    static let descriptor: Data = {
        var bytes: [UInt8] = [0x05,0x0d,0x09,0x04,0xa1,0x01,0x85,0x01]
        for _ in 0..<maximumContacts {
            bytes += [0x05,0x0d,0x09,0x22,0xa1,0x02,
                      0x09,0x42,0x09,0x32,0x09,0x47,0x09,0x34,0x15,0x00,0x25,0x01,0x75,0x01,0x95,0x04,0x81,0x02,
                      0x75,0x04,0x95,0x01,0x81,0x03,
                      0x09,0x51,0x15,0x00,0x25,0x0a,0x75,0x08,0x95,0x01,0x81,0x02,
                      0x05,0x01,0x09,0x30,0x09,0x31,0x15,0x00,0x26,0xff,0x7f,0x75,0x10,0x95,0x02,0x81,0x02,0xc0]
        }
        bytes += [0x05,0x0d,0x09,0x56,0x15,0x00,0x27,0xff,0xff,0x00,0x00,0x75,0x10,0x95,0x01,0x81,0x02,
                  0x09,0x54,0x25,0x0a,0x75,0x08,0x95,0x01,0x81,0x02,
                  0x85,0x02,0x09,0x55,0x25,0x0a,0x75,0x08,0x95,0x01,0xb1,0x02,0xc0]
        return Data(bytes)
    }()

    mutating func encode(_ frame: DesktopTouchFrame, scanTime: UInt16) throws -> Data {
        guard frame.contacts.count <= Self.maximumContacts,
              sequence.map({ frame.sequence > $0 }) ?? true else { throw invalid() }
        var next: [Int: DesktopTouchFrame.Contact] = [:]
        for contact in frame.contacts {
            guard (1...Self.maximumContacts).contains(contact.id), next[contact.id] == nil,
                  contact.x.isFinite, contact.y.isFinite, (0...1).contains(contact.x), (0...1).contains(contact.y),
                  ["began", "moved", "stationary", "ended", "cancelled"].contains(contact.phase),
                  (contact.phase == "began") == (active[contact.id] == nil) else { throw invalid() }
            next[contact.id] = contact
        }
        // Dropping an active ID would leave a finger pressed. Require its explicit lift.
        guard active.keys.allSatisfy({ next[$0] != nil }) else { throw invalid() }
        let report = Self.pack(frame.contacts, scanTime: scanTime)
        active = next.filter { $0.value.isActive }; sequence = frame.sequence
        return report
    }
    mutating func release(scanTime: UInt16) -> Data {
        let contacts = active.values.map { DesktopTouchFrame.Contact(id: $0.id, phase: "cancelled", x: $0.x, y: $0.y) }
        active.removeAll()
        return Self.pack(contacts, scanTime: scanTime)
    }
    static func pack(_ contacts: [DesktopTouchFrame.Contact], scanTime: UInt16) -> Data {
        var bytes = [UInt8](repeating: 0, count: byteCount)
        bytes[0] = 1
        for (slot, contact) in contacts.sorted(by: { $0.id < $1.id }).enumerated() {
            let offset = 1 + slot * 6
            // Touch + Untouch marks cancellation in Apple's HID digitizer driver.
            bytes[offset] = contact.phase == "cancelled" ? 0b1111 : (contact.isActive ? 0b111 : 0b100)
            bytes[offset + 1] = UInt8(contact.id)
            for (axis, value) in [contact.x, contact.y].enumerated() {
                let position = UInt16((value * 32767).rounded())
                bytes[offset + 2 + axis * 2] = UInt8(truncatingIfNeeded: position)
                bytes[offset + 3 + axis * 2] = UInt8(truncatingIfNeeded: position >> 8)
            }
        }
        bytes[byteCount - 3] = UInt8(truncatingIfNeeded: scanTime)
        bytes[byteCount - 2] = UInt8(truncatingIfNeeded: scanTime >> 8)
        bytes[byteCount - 1] = UInt8(contacts.count)
        return Data(bytes)
    }
    static func liftCancelled(_ contacts: [DesktopTouchFrame.Contact]) -> [DesktopTouchFrame.Contact] {
        contacts.map { $0.phase == "cancelled" ? .init(id: $0.id, phase: "ended", x: $0.x, y: $0.y) : $0 }
    }
    private func invalid() -> HostFailure { HostFailure(message: "原生触点数据或顺序无效，会话已停止。") }
}
