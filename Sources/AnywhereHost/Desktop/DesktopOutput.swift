import Foundation

/// All producers share one writer so control and video packets cannot interleave.
final class DesktopOutput: @unchecked Sendable {
    static let shared = DesktopOutput()
    private let lock = NSLock()
    private let handle: FileHandle
    init(handle: FileHandle = .standardOutput) { self.handle = handle }
    func write(_ type: UInt8, _ payload: Data) throws {
        try lock.withLock { try handle.write(contentsOf: DesktopWire.packet(type, payload)) }
    }
}
