// Audio and video with macOS's own AVFoundation, for when FFmpeg isn't
// installed. It covers what Macs make and play: MP4 and MOV video (H.264,
// HEVC, ProRes), and M4A, MP3, WAV, AIFF and FLAC sound.
//
//   radlet-vision media <input> <output> <to> [start=<s>] [end=<s>] [size=<preset>] [reencode]
//
//   to       mp4 | mov        video (a picture track is needed)
//            m4a              AAC sound, from sound or video
//            wav | aiff       16-bit PCM sound, from sound or video
//   start, end                keep only this part, in seconds (re-encoded,
//                             so the cuts are exact)
//   size     1920x1080 | 1280x720 | 960x540
//                             H.264 no larger than this (for compressing)
//   reencode                  never copy streams unchanged
//
// Without start, end, size or reencode, MP4 ↔ MOV copies the streams
// unchanged when the new container can hold them. Exit 3 means there was
// nothing to do: no sound to keep, or a compressed copy that came out no
// smaller. The engine side is crates/radlet-core/src/engines/avmedia.rs.

import AVFoundation
import CoreMedia
import Foundation

struct MediaRequest {
    var to = ""
    var start: Double?
    var end: Double?
    var size: String?
    var reencode = false

    init(_ words: [String]) throws {
        guard let first = words.first else { throw Failure.failed("no output format given") }
        to = first.lowercased()
        for word in words.dropFirst() {
            let parts = word.split(separator: "=", maxSplits: 1).map(String.init)
            switch (parts.first, parts.count) {
            case ("start", 2): start = Double(parts[1])
            case ("end", 2): end = Double(parts[1])
            case ("size", 2): size = parts[1]
            case ("reencode", 1): reencode = true
            default: throw Failure.failed("unknown media option \(word)")
            }
        }
    }

    var fileType: AVFileType? {
        switch to {
        case "mp4": return .mp4
        case "mov": return .mov
        case "m4a": return .m4a
        case "wav": return .wav
        case "aiff": return .aiff
        default: return nil
        }
    }

    /// The part to keep, clamped to the file's length.
    func range(in duration: CMTime) throws -> CMTimeRange? {
        guard start != nil || end != nil else { return nil }
        let length = duration.seconds
        let known = length.isFinite && length > 0
        let from = max(start ?? 0, 0)
        // An end past the file stops at the file's end.
        let until = known ? min(end ?? length, length) : (end ?? .infinity)
        if known, from >= length {
            throw Failure.failed(String(format: "the start (%.1f s) is past the end (%.1f s)", from, length))
        }
        guard until > from, until.isFinite else { throw Failure.failed("the part to keep is empty") }
        let scale: CMTimeScale = 600
        return CMTimeRange(start: CMTime(seconds: from, preferredTimescale: scale),
                           end: CMTime(seconds: until, preferredTimescale: scale))
    }
}

func media(_ args: [String]) throws {
    let input = URL(fileURLWithPath: args[0]), output = URL(fileURLWithPath: args[1])
    let request = try MediaRequest(Array(args.dropFirst(2)))
    guard let fileType = request.fileType else {
        throw Failure.failed("macOS can't write \(request.to.uppercased()) without FFmpeg")
    }
    let asset = AVURLAsset(url: input)
    let (duration, videoTracks, audioTracks) = try blocking {
        (try await asset.load(.duration),
         try await asset.loadTracks(withMediaType: .video),
         try await asset.loadTracks(withMediaType: .audio))
    }
    guard !videoTracks.isEmpty || !audioTracks.isEmpty else {
        throw Failure.failed("macOS can't read this file. Install FFmpeg (brew install ffmpeg) for more formats.")
    }
    let range = try request.range(in: duration)
    // The engine names the output; an export refuses to overwrite.
    try? FileManager.default.removeItem(at: output)

    switch fileType {
    case .wav, .aiff:
        guard let track = audioTracks.first else { throw Failure.nothingFound("this file has no sound") }
        try writePCM(asset, track: track, to: output, fileType: fileType, range: range, duration: duration)
    case .m4a:
        guard !audioTracks.isEmpty else { throw Failure.nothingFound("this file has no sound") }
        try exportAsset(asset, preset: AVAssetExportPresetAppleM4A, to: output, fileType: fileType, range: range)
    default:
        guard !videoTracks.isEmpty else { throw Failure.failed("there's no picture to make a video from") }
        if let size = request.size {
            let preset: String
            switch size {
            case "1920x1080": preset = AVAssetExportPreset1920x1080
            case "1280x720": preset = AVAssetExportPreset1280x720
            case "960x540": preset = AVAssetExportPreset960x540
            default: throw Failure.failed("unknown size \(size)")
            }
            try exportAsset(asset, preset: preset, to: output, fileType: fileType, range: range)
            try keepOnlyIfSmaller(output, than: input)
        } else if range == nil, !request.reencode,
                  let copy = AVAssetExportSession(asset: asset, presetName: AVAssetExportPresetPassthrough),
                  copy.supportedFileTypes.contains(fileType) {
            // Same streams, new container: fast and lossless. Some streams
            // turn out not to fit the container (ProRes in MP4, say): then
            // they're re-encoded.
            do {
                try runExport(copy, to: output, fileType: fileType, range: nil)
            } catch {
                try? FileManager.default.removeItem(at: output)
                try exportAsset(asset, preset: AVAssetExportPresetHighestQuality, to: output, fileType: fileType, range: nil)
            }
        } else {
            try exportAsset(asset, preset: AVAssetExportPresetHighestQuality, to: output, fileType: fileType, range: range)
        }
    }
}

func exportAsset(_ asset: AVAsset, preset: String, to output: URL, fileType: AVFileType, range: CMTimeRange?) throws {
    guard let session = AVAssetExportSession(asset: asset, presetName: preset) else {
        throw Failure.failed("macOS can't convert this file that way")
    }
    guard session.supportedFileTypes.contains(fileType) else {
        throw Failure.failed("macOS can't put this file's sound or picture in \(fileType.rawValue)")
    }
    try runExport(session, to: output, fileType: fileType, range: range)
}

/// Runs an export, reporting its progress, and waits for it.
func runExport(_ session: AVAssetExportSession, to output: URL, fileType: AVFileType, range: CMTimeRange?) throws {
    session.outputURL = output
    session.outputFileType = fileType
    session.shouldOptimizeForNetworkUse = fileType == .mp4 || fileType == .mov || fileType == .m4a
    if let range { session.timeRange = range }
    let box = AsyncBox<Bool>()
    session.exportAsynchronously {
        DispatchQueue.main.async { box.result = .success(true) }
    }
    var reported = -1.0
    while box.result == nil {
        RunLoop.main.run(mode: .default, before: Date().addingTimeInterval(0.1))
        let p = Double(session.progress)
        if p - reported >= 0.01 {
            report(p)
            reported = p
        }
    }
    switch session.status {
    case .completed:
        return
    case .cancelled:
        throw Failure.failed("the conversion was cancelled")
    default:
        let reason = session.error?.localizedDescription ?? "unknown error"
        throw Failure.failed("macOS couldn't convert this file (\(reason))")
    }
}

/// Writes the sound as 16-bit PCM (WAV little-endian, AIFF big-endian),
/// keeping its sample rate. More than two channels are mixed to stereo.
func writePCM(_ asset: AVAsset, track: AVAssetTrack, to output: URL, fileType: AVFileType,
              range: CMTimeRange?, duration: CMTime) throws {
    let descriptions = try blocking { try await track.load(.formatDescriptions) }
    let stream = descriptions.first.flatMap { CMAudioFormatDescriptionGetStreamBasicDescription($0)?.pointee }
    let rate = stream.map { $0.mSampleRate > 0 ? $0.mSampleRate : 44_100 } ?? 44_100
    let channels = min(max(Int(stream?.mChannelsPerFrame ?? 2), 1), 2)
    // Naming the layout lets the reader mix surround sound down to it.
    var layout = AudioChannelLayout()
    layout.mChannelLayoutTag = channels == 2 ? kAudioChannelLayoutTag_Stereo : kAudioChannelLayoutTag_Mono
    let settings: [String: Any] = [
        AVFormatIDKey: kAudioFormatLinearPCM,
        AVSampleRateKey: rate,
        AVNumberOfChannelsKey: channels,
        AVChannelLayoutKey: Data(bytes: &layout, count: MemoryLayout<AudioChannelLayout>.size),
        AVLinearPCMBitDepthKey: 16,
        AVLinearPCMIsFloatKey: false,
        AVLinearPCMIsBigEndianKey: fileType == .aiff,
        AVLinearPCMIsNonInterleaved: false,
    ]

    let reader = try AVAssetReader(asset: asset)
    if let range { reader.timeRange = range }
    let source = AVAssetReaderTrackOutput(track: track, outputSettings: settings)
    source.alwaysCopiesSampleData = false
    guard reader.canAdd(source) else { throw Failure.failed("macOS can't read this file's sound") }
    reader.add(source)

    let writer = try AVAssetWriter(outputURL: output, fileType: fileType)
    let sink = AVAssetWriterInput(mediaType: .audio, outputSettings: settings)
    sink.expectsMediaDataInRealTime = false
    guard writer.canAdd(sink) else { throw Failure.failed("macOS can't write this sound as \(fileType.rawValue)") }
    writer.add(sink)

    guard reader.startReading() else {
        throw Failure.failed("macOS can't read this file's sound (\(reader.error?.localizedDescription ?? "unknown error"))")
    }
    guard writer.startWriting() else {
        throw Failure.failed("couldn't write the sound (\(writer.error?.localizedDescription ?? "unknown error"))")
    }
    let origin = range?.start ?? .zero
    writer.startSession(atSourceTime: origin)
    let total = (range?.duration ?? duration).seconds
    var reported = -1.0
    while let buffer = source.copyNextSampleBuffer() {
        while !sink.isReadyForMoreMediaData {
            if writer.status == .failed || reader.status == .failed {
                let error = writer.error ?? reader.error
                throw Failure.failed("couldn't write the sound (\(error?.localizedDescription ?? "unknown error"))")
            }
            Thread.sleep(forTimeInterval: 0.002)
        }
        guard sink.append(buffer) else {
            throw Failure.failed("couldn't write the sound (\(writer.error?.localizedDescription ?? "unknown error"))")
        }
        if total > 0 {
            let done = (CMSampleBufferGetPresentationTimeStamp(buffer) - origin).seconds / total
            if done - reported >= 0.01 {
                report(done)
                reported = done
            }
        }
    }
    if reader.status == .failed {
        throw Failure.failed("macOS couldn't read all of the sound (\(reader.error?.localizedDescription ?? "unknown error"))")
    }
    sink.markAsFinished()
    try blocking { await writer.finishWriting() }
    guard writer.status == .completed else {
        throw Failure.failed("couldn't finish the sound file (\(writer.error?.localizedDescription ?? "unknown error"))")
    }
}

/// A compressed copy no smaller than the original isn't worth keeping.
func keepOnlyIfSmaller(_ output: URL, than input: URL) throws {
    let size = { (url: URL) in (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? Int) ?? 0 }
    if size(output) >= size(input) {
        try? FileManager.default.removeItem(at: output)
        throw Failure.nothingFound("it's already as small as this setting makes it")
    }
}

/// One line for the self-test: makes a second of sound, then converts it.
func mediaSelftest(in dir: URL) throws -> String {
    let wav = dir.appendingPathComponent("tone.wav")
    let rate = 44_100.0
    guard let format = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: rate, channels: 1, interleaved: true),
          let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(rate)),
          let samples = buffer.int16ChannelData?[0] else {
        throw Failure.failed("couldn't make the test sound")
    }
    buffer.frameLength = AVAudioFrameCount(rate)
    for i in 0..<Int(rate) {
        samples[i] = Int16(sin(Double(i) * 2 * .pi * 440 / rate) * 8_000)
    }
    do {
        let file = try AVAudioFile(forWriting: wav, settings: format.settings, commonFormat: .pcmFormatInt16, interleaved: true)
        try file.write(from: buffer)
    }
    let m4a = dir.appendingPathComponent("tone.m4a")
    try media([wav.path, m4a.path, "m4a"])
    let aiff = dir.appendingPathComponent("tone.aiff")
    try media([m4a.path, aiff.path, "aiff", "start=0.25", "end=0.75"])
    let length = try blocking { try await AVURLAsset(url: aiff).load(.duration) }.seconds
    guard abs(length - 0.5) < 0.05 else {
        throw Failure.failed(String(format: "the trimmed test sound is %.2f s long, not 0.5 s", length))
    }
    return "media: ok (WAV → M4A → trimmed AIFF)"
}
