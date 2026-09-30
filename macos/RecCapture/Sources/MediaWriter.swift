import AVFoundation
import AppKit
import CoreMedia
import CoreVideo
import Foundation

struct RecorderError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
    init(_ message: String) { self.message = message }
}

/// All methods are called on a single serial media queue (video AND audio).
final class MediaWriter {
    let writer: AVAssetWriter
    let video: AVAssetWriterInput
    let audio: AVAssetWriterInput?
    let adaptor: AVAssetWriterInputPixelBufferAdaptor
    let overlay: OverlayState
    let width: Int
    let height: Int
    private(set) var origin: CMTime?
    private(set) var lastVideoPTS: CMTime?
    private(set) var videoFrames = 0
    private(set) var audioSamples = 0

    init(url: URL, width: Int, height: Int, systemAudio: Bool, overlay: OverlayState, realtime: Bool = true) throws {
        self.width = width; self.height = height; self.overlay = overlay
        writer = try AVAssetWriter(outputURL: url, fileType: .mp4)
        video = AVAssetWriterInput(mediaType: .video, outputSettings: [
            AVVideoCodecKey: AVVideoCodecType.h264,
            AVVideoWidthKey: width, AVVideoHeightKey: height,
            AVVideoCompressionPropertiesKey: [AVVideoAverageBitRateKey: max(2_000_000, width * height * 3), AVVideoExpectedSourceFrameRateKey: 30],
        ])
        video.expectsMediaDataInRealTime = realtime
        adaptor = AVAssetWriterInputPixelBufferAdaptor(assetWriterInput: video, sourcePixelBufferAttributes: [
            kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
            kCVPixelBufferWidthKey as String: width, kCVPixelBufferHeightKey as String: height,
            kCVPixelBufferIOSurfacePropertiesKey as String: [:],
        ])
        guard writer.canAdd(video) else { throw RecorderError("cannot add H.264 input") }
        writer.add(video)
        if systemAudio {
            let input = AVAssetWriterInput(mediaType: .audio, outputSettings: [
                AVFormatIDKey: kAudioFormatMPEG4AAC, AVSampleRateKey: 48_000,
                AVNumberOfChannelsKey: 2, AVEncoderBitRateKey: 128_000,
            ])
            input.expectsMediaDataInRealTime = realtime
            guard writer.canAdd(input) else { throw RecorderError("cannot add AAC system audio input") }
            writer.add(input); audio = input
        } else { audio = nil }
    }
    @discardableResult
    func appendVideo(_ pixel: CVPixelBuffer, at pts: CMTime) throws -> Bool {
        if origin == nil {
            guard writer.startWriting() else { throw writer.error ?? RecorderError("startWriting failed") }
            writer.startSession(atSourceTime: pts); origin = pts
        }
        guard writer.status == .writing else { throw writer.error ?? RecorderError("writer is not recording") }
        if let last = lastVideoPTS, CMTimeCompare(pts, last) <= 0 { return false }
        guard video.isReadyForMoreMediaData else { return false }
        guard let composed = composite(pixel) else { throw RecorderError("overlay composition failed") }
        guard adaptor.append(composed, withPresentationTime: pts) else { throw writer.error ?? RecorderError("video append failed") }
        videoFrames += 1; lastVideoPTS = pts
        return true
    }
    @discardableResult
    func appendAudio(_ sample: CMSampleBuffer) throws -> Bool {
        guard let audio, let origin else { return false }
        let pts = CMSampleBufferGetPresentationTimeStamp(sample)
        guard pts.isValid, CMTimeCompare(pts, origin) >= 0 else { return false }
        guard writer.status == .writing else { throw writer.error ?? RecorderError("audio writer failed") }
        guard audio.isReadyForMoreMediaData else { return false }
        guard audio.append(sample) else { throw writer.error ?? RecorderError("audio append failed") }
        audioSamples += CMSampleBufferGetNumSamples(sample)
        return true
    }
    func finish(at end: CMTime, completion: @escaping (Error?) -> Void) {
        guard videoFrames > 0 else { writer.cancelWriting(); completion(RecorderError("no video frames captured")); return }
        if audio != nil && audioSamples == 0 {
            writer.cancelWriting(); completion(RecorderError("system audio requested but no audio samples received")); return
        }
        guard writer.status == .writing else { completion(writer.error ?? RecorderError("writer failed before finalization")); return }
        writer.endSession(atSourceTime: end)
        video.markAsFinished(); audio?.markAsFinished()
        writer.finishWriting { [self] in
            completion(writer.status == .completed ? nil : (writer.error ?? RecorderError("MP4 finalization failed")))
        }
    }
    func composite(_ source: CVPixelBuffer) -> CVPixelBuffer? {
        let w = CVPixelBufferGetWidth(source), h = CVPixelBufferGetHeight(source)
        guard w == width && h == height else { return nil }
        guard let target = makePixelBuffer(width: w, height: h) else { return nil }
        CVPixelBufferLockBaseAddress(source, .readOnly); CVPixelBufferLockBaseAddress(target, [])
        defer { CVPixelBufferUnlockBaseAddress(source, .readOnly); CVPixelBufferUnlockBaseAddress(target, []) }
        guard let from = CVPixelBufferGetBaseAddress(source), let to = CVPixelBufferGetBaseAddress(target) else { return nil }
        let sourceStride = CVPixelBufferGetBytesPerRow(source), targetStride = CVPixelBufferGetBytesPerRow(target)
        for row in 0..<h { memcpy(to.advanced(by: row * targetStride), from.advanced(by: row * sourceStride), min(sourceStride, targetStride)) }
        guard let context = CGContext(data: to, width: w, height: h, bitsPerComponent: 8, bytesPerRow: targetStride,
            space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue) else { return nil }
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: true)
        drawOverlay(size: NSSize(width: w, height: h), state: overlay.snapshot())
        NSGraphicsContext.restoreGraphicsState()
        return target
    }
}

func makePixelBuffer(width: Int, height: Int) -> CVPixelBuffer? {
    var pixel: CVPixelBuffer?
    let attributes: [CFString: Any] = [kCVPixelBufferIOSurfacePropertiesKey: [:], kCVPixelBufferCGImageCompatibilityKey: true, kCVPixelBufferCGBitmapContextCompatibilityKey: true]
    guard CVPixelBufferCreate(kCFAllocatorDefault, width, height, kCVPixelFormatType_32BGRA, attributes as CFDictionary, &pixel) == kCVReturnSuccess else { return nil }
    return pixel
}
