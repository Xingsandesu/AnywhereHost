import Foundation
import Testing
@testable import AnywhereHost

struct DesktopTouchTests {
    private func contact(_ id: Int, _ phase: String, x: Double = 0.25, y: Double = 0.75) -> DesktopTouchFrame.Contact {
        .init(id: id, phase: phase, x: x, y: y)
    }
    @Test func simultaneousContactsKeepIdentityAndExplicitLift() throws {
        var report = DesktopTouchReport()
        let first = try report.encode(.init(sequence: 1, contacts: [contact(2, "began"), contact(1, "began")]), scanTime: 123)
        #expect(first.count == 64 && first[0] == 1 && first[63] == 2)
        #expect(first[1] == 7 && first[2] == 1 && first[8] == 2)
        #expect(first[61] == 123 && first[62] == 0)
        let next = try report.encode(.init(sequence: 2, contacts: [contact(1, "stationary"), contact(2, "ended")]), scanTime: 124)
        #expect(next[1] == 7 && next[7] == 4 && next[8] == 2)
        #expect(Set(report.active.keys) == [1])
        _ = try report.encode(.init(sequence: 3, contacts: [contact(1, "moved"), contact(2, "began")]), scanTime: 125)
        let release = report.release(scanTime: 126)
        #expect(release[1] == 15 && release[7] == 15 && release[63] == 2)
        #expect(report.active.isEmpty)
    }
    @Test func cancellationCarriesCancelMaskThenClearsOnlyCancelledContacts() throws {
        var report = DesktopTouchReport()
        _ = try report.encode(.init(sequence: 1, contacts: [contact(1, "began"), contact(2, "began")]), scanTime: 0)
        let contacts = [contact(1, "cancelled"), contact(2, "stationary")]
        let cancel = try report.encode(.init(sequence: 2, contacts: contacts), scanTime: 1)
        #expect(cancel[1] == 15 && cancel[7] == 7)
        let settled = DesktopTouchReport.pack(DesktopTouchReport.liftCancelled(contacts), scanTime: 2)
        #expect(settled[1] == 4 && settled[7] == 7)
        #expect(Set(report.active.keys) == [2])
    }
    @Test func rejectsDisappearingReplayedAndMalformedContactsAtomically() throws {
        var report = DesktopTouchReport()
        _ = try report.encode(.init(sequence: 1, contacts: [contact(1, "began")]), scanTime: 0)
        for contacts in [[], [contact(1, "began")], [contact(1, "moved"), contact(1, "moved")],
                         [contact(1, "moved", x: .nan)], [contact(1, "moved", y: 1.1)],
                         [contact(1, "moved"), contact(11, "began")], [contact(1, "invalid")]] {
            #expect(throws: HostFailure.self) { try report.encode(.init(sequence: 2, contacts: contacts), scanTime: 0) }
            #expect(Set(report.active.keys) == [1])
        }
        #expect(throws: HostFailure.self) { try report.encode(.init(sequence: 1, contacts: [contact(1, "ended")]), scanTime: 0) }
        _ = try report.encode(.init(sequence: 2, contacts: [contact(1, "cancelled")]), scanTime: 0)
        #expect(report.active.isEmpty)
    }
    @Test func tenFingersAndCoordinateExtremesFitOneReport() throws {
        var report = DesktopTouchReport()
        let frame = DesktopTouchFrame(sequence: 1, contacts: (1...10).map { contact($0, "began", x: 0, y: 1) })
        let bytes = try report.encode(frame, scanTime: .max)
        #expect(bytes[63] == 10)
        for slot in 0..<10 {
            let offset = 1 + slot * 6
            #expect(bytes[offset + 1] == UInt8(slot + 1))
            #expect(bytes[offset + 2] == 0 && bytes[offset + 3] == 0)
            #expect(bytes[offset + 4] == 255 && bytes[offset + 5] == 127)
        }
    }
    @Test func descriptorReportLengthsMatchWirePayloads() {
        // Parse HID short items independently to catch report size/count or ID drift.
        let bytes = Array(DesktopTouchReport.descriptor)
        var offset = 0, size = 0, count = 0, id = 0
        var inputs: [Int: Int] = [:], features: [Int: Int] = [:]
        while offset < bytes.count {
            let prefix = Int(bytes[offset]); offset += 1
            let width = [0, 1, 2, 4][prefix & 3]
            var value = 0
            for index in 0..<width { value |= Int(bytes[offset + index]) << (index * 8) }
            offset += width
            switch prefix & 0xfc {
            case 0x74: size = value
            case 0x94: count = value
            case 0x84: id = value
            case 0x80: inputs[id, default: 0] += size * count
            case 0xb0: features[id, default: 0] += size * count
            default: break
            }
        }
        #expect(inputs == [1: (DesktopTouchReport.byteCount - 1) * 8])
        #expect(features == [2: 8])
    }
}
