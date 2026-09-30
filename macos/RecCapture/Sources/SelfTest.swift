import AudioToolbox
import AVFoundation
import AppKit
import CoreMedia
import CoreVideo
import Foundation

/// Synthetic PCM for the CI codec test. This does NOT request screen or microphone access.
func syntheticAudio(at pts: CMTime, frames: Int = 1600) throws -> CMSampleBuffer {
    var asbd = AudioStreamBasicDescription(mSampleRate: 48000, mFormatID: kAudioFormatLinearPCM,
        mFormatFlags: kLinearPCMFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked,
        mBytesPerPacket: 4, mFramesPerPacket: 1, mBytesPerFrame: 4, mChannelsPerFrame: 2, mBitsPerChannel: 16, mReserved: 0)
    var format: CMAudioFormatDescription?
    guard CMAudioFormatDescriptionCreate(allocator: kCFAllocatorDefault, asbd: &asbd, layoutSize: 0, layout: nil,
        magicCookieSize: 0, magicCookie: nil, extensions: nil, formatDescriptionOut: &format) == noErr, let format else { throw RecorderError("synthetic audio format failed") }
    var samples = [Int16](repeating: 0, count: frames * 2)
    let start = CMTimeGetSeconds(pts)
    for i in 0..<frames {
        let value = Int16(sin((start + Double(i) / 48000) * 2 * .pi * 440) * 10000)
        samples[i * 2] = value; samples[i * 2 + 1] = value
    }
    var block: CMBlockBuffer?
    let count = samples.count * MemoryLayout<Int16>.size
    guard CMBlockBufferCreateWithMemoryBlock(allocator: kCFAllocatorDefault, memoryBlock: nil, blockLength: count,
        blockAllocator: kCFAllocatorDefault, customBlockSource: nil, offsetToData: 0, dataLength: count, flags: 0,
        blockBufferOut: &block) == kCMBlockBufferNoErr, let block else { throw RecorderError("synthetic PCM allocation failed") }
    let copied = samples.withUnsafeBytes { CMBlockBufferReplaceDataBytes(with: $0.baseAddress!, blockBuffer: block, offsetIntoDestination: 0, dataLength: count) }
    guard copied == noErr else { throw RecorderError("synthetic PCM copy failed") }
    var sample: CMSampleBuffer?
    guard CMAudioSampleBufferCreateReadyWithPacketDescriptions(allocator: kCFAllocatorDefault, dataBuffer: block,
        formatDescription: format, sampleCount: frames, presentationTimeStamp: pts, packetDescriptions: nil,
        sampleBufferOut: &sample) == noErr, let sample else { throw RecorderError("synthetic audio sample failed") }
    return sample
}

func syntheticRecording(_ args: Args) throws {
    guard let output = args.output else { throw RecorderError("self-test requires --output") }
    let state = OverlayState(); state.runId = "TEST"
    state.apply(json: ["cmd": "hud", "action": "Synthetic codec test — NOT a screen recording", "verdict": NSNull()])
    let url = URL(fileURLWithPath: output)
    try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
    let writer = try MediaWriter(url: url, width: 640, height: 360, systemAudio: args.systemAudio, overlay: state, realtime: false)
    guard let pixel = makePixelBuffer(width: 640, height: 360) else { throw RecorderError("synthetic frame allocation failed") }
    CVPixelBufferLockBaseAddress(pixel, [])
    if let base = CVPixelBufferGetBaseAddress(pixel) {
        let bytes = base.assumingMemoryBound(to: UInt8.self), stride = CVPixelBufferGetBytesPerRow(pixel)
        for y in 0..<360 { for x in 0..<640 { let offset = y * stride + x * 4; bytes[offset] = 80; bytes[offset + 1] = 80; bytes[offset + 2] = 80; bytes[offset + 3] = 255 } }
    }
    CVPixelBufferUnlockBaseAddress(pixel, [])
    let deadline = Date().addingTimeInterval(20)
    for i in 0..<30 {
        let pts = CMTime(value: Int64(i), timescale: 30)
        while try !writer.appendVideo(pixel, at: pts) {
            guard Date() < deadline else { throw RecorderError("synthetic video backpressure timeout") }; Thread.sleep(forTimeInterval: 0.01)
        }
        if args.systemAudio {
            let sample = try syntheticAudio(at: pts)
            while try !writer.appendAudio(sample) {
                guard Date() < deadline else { throw RecorderError("synthetic audio backpressure timeout") }; Thread.sleep(forTimeInterval: 0.01)
            }
        }
    }
    let done = DispatchSemaphore(value: 0); var failure: Error?
    writer.finish(at: CMTime(value: 1, timescale: 1)) { failure = $0; done.signal() }
    guard done.wait(timeout: .now() + 20) == .success else { throw RecorderError("synthetic writer finalization timeout") }
    if let failure { throw failure }
    emit(["event": "self-test-complete", "video_frames": writer.videoFrames, "audio_samples": writer.audioSamples, "system_audio": args.systemAudio, "path": output])
}
