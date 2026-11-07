import Foundation

/// v1: big-endian UInt32 length (including type), UInt8 type, payload.
/// 1 = H.264 configuration JSON, 2 = AVCC access unit, 3 = input JSON, 4 = error UTF-8.
enum DesktopWire {
    static let limit = 8 * 1024 * 1024
    static func packet(_ type: UInt8, _ payload: Data) -> Data {
        var size = UInt32(payload.count + 1).bigEndian
        var data = withUnsafeBytes(of: &size) { Data($0) }
        data.append(type); data.append(payload)
        return data
    }
    static func read(_ handle: FileHandle, count: Int) throws -> Data {
        var result = Data()
        while result.count < count {
            guard let bytes = try handle.read(upToCount: count - result.count), !bytes.isEmpty else {
                throw CocoaError(.fileReadUnknown)
            }
            result.append(bytes)
        }
        return result
    }
    static func receive(_ handle: FileHandle, maximumLength: Int = limit) throws -> (UInt8, Data) {
        let header = try read(handle, count: 4)
        let count = header.reduce(0) { ($0 << 8) | Int($1) }
        guard (1...maximumLength).contains(count) else { throw CocoaError(.fileReadCorruptFile) }
        let body = try read(handle, count: count)
        return (body[0], Data(body.dropFirst()))
    }
}
