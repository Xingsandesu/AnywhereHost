import Foundation
import ScreenCaptureKit
import VideoToolbox

/// Capture/encode/write are bounded by acknowledgements. Never drop an encoded reference frame.
final class DesktopEncoder: NSObject, SCStreamOutput, @unchecked Sendable {
    let queue = DispatchQueue(label: "Anywhere.desktop.encode", qos: .userInteractive)
    private var encoder: VTCompressionSession?
    private let lock = NSLock()
    private var pending = 0
    private var stopped = false
    private var latestSample: CMSampleBuffer?
    private let width: Int
    private let height: Int
    private let output: DesktopOutput
    private let generation: Int
    init(width: Int, height: Int, generation: Int = 0, output: DesktopOutput = .shared) throws {
        self.output = output
        self.generation = generation
        self.width = width; self.height = height
        super.init()
        let status = VTCompressionSessionCreate(allocator: nil, width: Int32(width), height: Int32(height), codecType: kCMVideoCodecType_H264,
            encoderSpecification: [kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder: true] as CFDictionary,
            imageBufferAttributes: nil, compressedDataAllocator: nil, outputCallback: { ref, _, status, _, sample in
                guard let ref else { return }
                let owner = Unmanaged<DesktopEncoder>.fromOpaque(ref).takeUnretainedValue()
                guard status == noErr, let sample else { owner.failed("硬件视频编码失败：\(status)"); return }
                owner.write(sample)
            }, refcon: Unmanaged.passUnretained(self).toOpaque(), compressionSessionOut: &encoder)
        guard status == noErr, let encoder else { throw HostFailure(message: "无法启动硬件视频编码：\(status)") }
        for (key, value): (CFString, CFTypeRef) in [
            (kVTCompressionPropertyKey_RealTime, kCFBooleanTrue),
            (kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse),
            (kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_High_AutoLevel),
            (kVTCompressionPropertyKey_ExpectedFrameRate, 60 as CFNumber),
            (kVTCompressionPropertyKey_MaxKeyFrameInterval, 60 as CFNumber),
            (kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, 1 as CFNumber),
            (kVTCompressionPropertyKey_AverageBitRate, min(40_000_000, max(8_000_000, width * height * 6)) as CFNumber)
        ] {
            let result = VTSessionSetProperty(encoder, key: key, value: value)
            guard result == noErr else { throw HostFailure(message: "视频编码设置失败：\(result)") }
        }
        guard VTCompressionSessionPrepareToEncodeFrames(encoder) == noErr else { throw HostFailure(message: "视频编码器未就绪。") }
    }
    func acknowledge() {
        lock.withLock { pending = max(0, pending - 1) }
        queue.async { [self] in
            if let sample = latestSample { latestSample = nil; encode(sample) }
        }
    }
    func stream(_ stream: SCStream, didOutputSampleBuffer sample: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen else { return }
        encode(sample)
    }
    func encode(_ sample: CMSampleBuffer) {
        guard sample.isValid, let image = sample.imageBuffer, let encoder else { return }
        let canEncode = lock.withLock {
            if stopped || pending >= 2 { return false }
            pending += 1; return true
        }
        guard canEncode else { latestSample = sample; return }
        latestSample = nil
        let status = VTCompressionSessionEncodeFrame(encoder, imageBuffer: image, presentationTimeStamp: sample.presentationTimeStamp, duration: CMTime(value: 1, timescale: 60), frameProperties: nil, sourceFrameRefcon: nil, infoFlagsOut: nil)
        if status != noErr { failed("无法编码桌面画面：\(status)") }
    }
    private func write(_ sample: CMSampleBuffer) {
        guard !lock.withLock({ stopped }), let format = sample.formatDescription, let buffer = sample.dataBuffer else { return }
        do {
            let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: false) as? [[CFString: Any]]
            if attachments?.first?[kCMSampleAttachmentKey_NotSync] as? Bool != true {
                var parameters: [String] = []
                for index in 0..<2 {
                    var pointer: UnsafePointer<UInt8>?, size = 0, count = 0
                    let status = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format, parameterSetIndex: index, parameterSetPointerOut: &pointer, parameterSetSizeOut: &size, parameterSetCountOut: &count, nalUnitHeaderLengthOut: nil)
                    guard status == noErr, let pointer else { throw HostFailure(message: "视频参数无效。") }
                    parameters.append(Data(bytes: pointer, count: size).base64EncodedString())
                }
                try output.write(1, JSONSerialization.data(withJSONObject: ["version": 1, "codec": "h264", "width": width, "height": height, "generation": generation, "parameters": parameters]))
            }
            let count = CMBlockBufferGetDataLength(buffer)
            guard count > 0, count < DesktopWire.limit else { throw HostFailure(message: "视频帧超出限制。") }
            var data = Data(count: count)
            let result = data.withUnsafeMutableBytes { CMBlockBufferCopyDataBytes(buffer, atOffset: 0, dataLength: count, destination: $0.baseAddress!) }
            guard result == noErr else { throw HostFailure(message: "无法读取视频帧。") }
            try output.write(2, data)
        } catch { failed(error.localizedDescription) }
    }
    func failed(_ message: String) {
        lock.withLock { stopped = true }
        try? output.write(4, Data(message.utf8))
        // Input release and display teardown must happen on the main actor.
        Task { @MainActor in NativeDesktopSession.current?.finish() }
    }
    func completeFrames() {
        if let encoder { VTCompressionSessionCompleteFrames(encoder, untilPresentationTimeStamp: .invalid) }
    }
    func stop() {
        lock.withLock { stopped = true }
        queue.sync {
            if let encoder { VTCompressionSessionInvalidate(encoder) }
            encoder = nil
        }
    }
}
