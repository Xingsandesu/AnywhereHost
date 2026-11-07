import Foundation
import Testing
import CoreMedia
import CoreVideo
import CoreGraphics
@testable import AnywhereHost

struct DesktopNativeTests {
    @Test func hardwareEncoderProducesFramedAVCAndObeysBackpressure() throws {
        let url = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
        FileManager.default.createFile(atPath: url.path, contents: nil)
        defer { try? FileManager.default.removeItem(at: url) }
        let file = try FileHandle(forWritingTo: url)
        defer { try? file.close() }
        let encoder = try DesktopEncoder(width: 64, height: 64, generation: 7, output: DesktopOutput(handle: file))
        defer { encoder.stop() }
        var pixel: CVPixelBuffer?
        #expect(CVPixelBufferCreate(nil, 64, 64, kCVPixelFormatType_32BGRA, [kCVPixelBufferIOSurfacePropertiesKey: [:]] as CFDictionary, &pixel) == kCVReturnSuccess)
        let buffer = try #require(pixel)
        CVPixelBufferLockBaseAddress(buffer, [])
        let base = try #require(CVPixelBufferGetBaseAddress(buffer)).assumingMemoryBound(to: UInt8.self)
        for y in 0..<64 {
            for x in 0..<64 {
                let offset = y * CVPixelBufferGetBytesPerRow(buffer) + x * 4
                base[offset] = 0; base[offset + 1] = 0; base[offset + 2] = 255; base[offset + 3] = 255
            }
        }
        CVPixelBufferUnlockBaseAddress(buffer, [])
        var format: CMFormatDescription?
        #expect(CMVideoFormatDescriptionCreateForImageBuffer(allocator: nil, imageBuffer: buffer, formatDescriptionOut: &format) == noErr)
        for index in 0..<5 {
            var timing = CMSampleTimingInfo(duration: CMTime(value: 1, timescale: 60), presentationTimeStamp: CMTime(value: Int64(index), timescale: 60), decodeTimeStamp: .invalid)
            var sample: CMSampleBuffer?
            #expect(CMSampleBufferCreateReadyWithImageBuffer(allocator: nil, imageBuffer: buffer, formatDescription: try #require(format), sampleTiming: &timing, sampleBufferOut: &sample) == noErr)
            encoder.encode(try #require(sample))
        }
        encoder.completeFrames()
        let beforeAck = try Data(contentsOf: url)
        func messages(_ data: Data) throws -> [(UInt8, Data)] {
            let path = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
            try data.write(to: path); defer { try? FileManager.default.removeItem(at: path) }
            let reader = try FileHandle(forReadingFrom: path); defer { try? reader.close() }
            var result: [(UInt8, Data)] = []
            while try reader.offset() < data.count { result.append(try DesktopWire.receive(reader)) }
            return result
        }
        let initial = try messages(beforeAck)
        #expect(initial.filter { $0.0 == 2 }.count == 2, "Only two unacknowledged frames may be encoded")
        let configuration = try #require(initial.first { $0.0 == 1 })
        let json = try JSONSerialization.jsonObject(with: configuration.1) as? [String: Any]
        #expect(json?["width"] as? Int == 64)
        #expect(json?["generation"] as? Int == 7)
        #expect((json?["parameters"] as? [String])?.count == 2)
        encoder.acknowledge()
        encoder.queue.sync { encoder.completeFrames() }
        #expect(try messages(Data(contentsOf: url)).filter { $0.0 == 2 }.count == 3, "The latest pending frame must arrive even when capture becomes idle")
    }
    @Test func malformedInputLengthsFailBeforeAllocating() throws {
        for data in [Data([0,0,0,0]), Data([255,255,255,255])] {
            let pipe = Pipe(); try pipe.fileHandleForWriting.write(contentsOf: data); try pipe.fileHandleForWriting.close()
            #expect(throws: (any Error).self) { try DesktopWire.receive(pipe.fileHandleForReading) }
            try pipe.fileHandleForReading.close()
        }
    }
}


@MainActor struct DesktopInputTests {
    @Test func scrollingTargetsTheRemotePointer() throws {
        var events: [CGEvent] = []
        let input = DesktopInput { events.append($0) }
        input.receive(["event": "pointer", "phase": "move", "x": 0.3, "y": 0.6])
        let point = try #require(events.last).location
        input.receive(["event": "scroll", "dx": 0.0, "dy": -80.0])
        #expect(events.last?.type == .scrollWheel)
        #expect(events.last?.location == point)
    }
    @Test func foldingKeepsViewportAspectAndCodecBounds() {
        for (width, height) in [(820, 1550), (1790, 1260), (1260, 1790), (4000, 3000)] {
            let size = NativeDesktopSession.requestedSize(["width": width, "height": height], extended: true)
            let expectedRatio = Double(min(3840, width)) / Double(min(2880, height))
            #expect(abs(size.width / size.height - expectedRatio) < 0.003)
            #expect(size.width <= 3840 && size.height <= 2880)
            #expect(Int(size.width) % 2 == 0 && Int(size.height) % 2 == 0)
        }
    }
    @Test func disconnectReleasesOnlyOwnedKeysAndButtons() {
        var events: [CGEvent] = []
        let input = DesktopInput { events.append($0) }
        input.receive(["event": "key", "code": 55, "down": true])
        input.receive(["event": "key", "code": 8, "down": true])
        input.receive(["event": "pointer", "phase": "down", "x": 0.5, "y": 0.5])
        #expect(events.last?.flags.contains(.maskCommand) == true)
        input.release()
        #expect(events.filter { $0.type == .keyUp && $0.getIntegerValueField(.keyboardEventKeycode) == 8 }.count == 1)
        #expect(events.contains { $0.type == .flagsChanged && !$0.flags.contains(.maskCommand) })
        #expect(events.last?.type == .leftMouseUp)
        #expect(events.last?.flags.contains(.maskCommand) == false)
        events.removeAll(); input.release()
        #expect(events.isEmpty)
    }
    @Test func pencilCarriesPressureTiltAndProximity() {
        var events: [CGEvent] = []
        let input = DesktopInput { events.append($0) }
        input.receive(["event": "pointer", "phase": "down", "kind": "pencil", "x": 0.5, "y": 0.5, "pressure": 0.7, "tiltX": -0.3, "tiltY": 0.4])
        #expect(events.first?.type == .tabletProximity)
        #expect(events.last?.getIntegerValueField(.mouseEventSubtype) == 1)
        #expect(abs((events.last?.getDoubleValueField(.tabletEventPointPressure) ?? 0) - 0.7) < 0.001)
        input.receive(["event": "pointer", "phase": "up", "kind": "pencil", "x": 0.5, "y": 0.5])
        #expect(events.last?.getIntegerValueField(.tabletProximityEventEnterProximity) == 0)
    }
}
