// Video jobs beyond plain conversion, with AVFoundation, for Macs without
// FFmpeg: moving pictures into video, compressing to a file size, joining
// clips and burning in subtitles.
//
//   radlet-vision media <in.gif> <output> mp4|mov frames
//   radlet-vision media <input> <output> mp4 target=<bytes>
//   radlet-vision join <output> mp4|mov|m4a <input>...
//   radlet-vision burn <video> <cues.json> <output> mp4|mov
//
// cues.json is a list of {"s": start, "e": end, "t": text}, in seconds.
// The engine side is crates/radlet-core/src/engines/avmedia.rs.

import AVFoundation
import CoreImage
import CoreMedia
import CoreText
import CoreVideo
import Foundation
import ImageIO
import UniformTypeIdentifiers

// MARK: - Sizes

/// Even and at least 2, as H.264 needs.
func evenSide(_ x: Double) -> Int { max(2, Int(x.rounded()) / 2 * 2) }

/// `size` shrunk so its longer side is at most `longest` (never enlarged),
/// with even sides.
func fitted(_ size: CGSize, longest: Double) -> CGSize {
    let scale = min(1, longest / max(Double(size.width), Double(size.height), 1))
    return CGSize(width: evenSide(Double(size.width) * scale), height: evenSide(Double(size.height) * scale))
}

/// A video track's picture size once it's turned upright.
func uprightVideoSize(_ natural: CGSize, _ transform: CGAffineTransform) -> CGSize {
    let r = CGRect(origin: .zero, size: natural).applying(transform)
    return CGSize(width: abs(r.width), height: abs(r.height))
}

/// Turns a track's frames upright and fits them, centred, into `render`.
func fitTransform(_ natural: CGSize, _ transform: CGAffineTransform, into render: CGSize) -> CGAffineTransform {
    let r = CGRect(origin: .zero, size: natural).applying(transform)
    guard r.width > 0, r.height > 0 else { return transform }
    let scale = min(render.width / r.width, render.height / r.height)
    let dx = (render.width - r.width * scale) / 2, dy = (render.height - r.height * scale) / 2
    return transform
        .concatenating(CGAffineTransform(translationX: -r.minX, y: -r.minY))
        .concatenating(CGAffineTransform(scaleX: scale, y: scale))
        .concatenating(CGAffineTransform(translationX: dx, y: dy))
}

/// One frame's length at a track's frame rate (30 when it's unknown, at
/// most 60 a second).
func frameDuration(_ rate: Float) -> CMTime {
    let fps: Int32 = rate.isFinite && rate > 1 ? min(Int32(rate.rounded()), 60) : 30
    return CMTime(value: 1, timescale: fps)
}

func fileSize(_ url: URL) -> Int64 {
    ((try? FileManager.default.attributesOfItem(atPath: url.path)[.size]) as? NSNumber)?.int64Value ?? 0
}

func writerFailure(_ writer: AVAssetWriter) -> Failure {
    .failed("couldn't write the video (\(writer.error?.localizedDescription ?? "unknown error"))")
}

// MARK: - Moving pictures

/// How long frame `i` of a GIF, animated PNG or WebP shows, in seconds.
/// Browsers show very short delays as a tenth of a second; so does this.
func frameDelay(_ source: CGImageSource, _ i: Int) -> Double {
    let props = CGImageSourceCopyPropertiesAtIndex(source, i, nil) as? [CFString: Any] ?? [:]
    let kinds: [(CFString, CFString, CFString)] = [
        (kCGImagePropertyGIFDictionary, kCGImagePropertyGIFUnclampedDelayTime, kCGImagePropertyGIFDelayTime),
        (kCGImagePropertyPNGDictionary, kCGImagePropertyAPNGUnclampedDelayTime, kCGImagePropertyAPNGDelayTime),
        (kCGImagePropertyWebPDictionary, kCGImagePropertyWebPUnclampedDelayTime, kCGImagePropertyWebPDelayTime),
    ]
    for (dictionary, unclamped, clamped) in kinds {
        guard let d = props[dictionary] as? [CFString: Any] else { continue }
        let delay = (d[unclamped] as? NSNumber)?.doubleValue ?? (d[clamped] as? NSNumber)?.doubleValue ?? 0
        return delay < 0.02 ? 0.1 : delay
    }
    return 0.1
}

/// Draws a picture over white, filling a 32-bit BGRA pixel buffer.
func draw(_ image: CGImage, into buffer: CVPixelBuffer) throws {
    CVPixelBufferLockBaseAddress(buffer, [])
    defer { CVPixelBufferUnlockBaseAddress(buffer, []) }
    let w = CVPixelBufferGetWidth(buffer), h = CVPixelBufferGetHeight(buffer)
    let info = CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue
    guard let base = CVPixelBufferGetBaseAddress(buffer), let space = CGColorSpace(name: CGColorSpace.sRGB),
          let ctx = CGContext(data: base, width: w, height: h, bitsPerComponent: 8,
                              bytesPerRow: CVPixelBufferGetBytesPerRow(buffer), space: space, bitmapInfo: info) else {
        throw Failure.failed("couldn't draw a frame")
    }
    ctx.setFillColor(CGColor(gray: 1, alpha: 1))
    ctx.fill(CGRect(x: 0, y: 0, width: w, height: h))
    ctx.interpolationQuality = .high
    ctx.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
}

/// Every frame of a GIF (or animated PNG or WebP) as H.264 video, each
/// shown for as long as the picture says. See-through parts turn white.
func framesToVideo(_ input: URL, _ output: URL, fileType: AVFileType) throws {
    guard let source = CGImageSourceCreateWithURL(input as CFURL, nil), CGImageSourceGetCount(source) > 0,
          let first = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
        throw Failure.failed("couldn't read this picture")
    }
    let count = CGImageSourceGetCount(source)
    let size = fitted(CGSize(width: first.width, height: first.height), longest: 3840)
    let w = Int(size.width), h = Int(size.height)
    let writer = try AVAssetWriter(outputURL: output, fileType: fileType)
    let compression: [String: Any] = [
        AVVideoAverageBitRateKey: max(600_000, w * h * 4),
        AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
    ]
    let sink = AVAssetWriterInput(mediaType: .video, outputSettings: [
        AVVideoCodecKey: AVVideoCodecType.h264,
        AVVideoWidthKey: w,
        AVVideoHeightKey: h,
        AVVideoCompressionPropertiesKey: compression,
    ])
    sink.expectsMediaDataInRealTime = false
    let adaptor = AVAssetWriterInputPixelBufferAdaptor(assetWriterInput: sink, sourcePixelBufferAttributes: [
        kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
        kCVPixelBufferWidthKey as String: w,
        kCVPixelBufferHeightKey as String: h,
    ])
    guard writer.canAdd(sink) else { throw Failure.failed("macOS can't write this video") }
    writer.add(sink)
    guard writer.startWriting() else { throw writerFailure(writer) }
    writer.startSession(atSourceTime: .zero)
    var time = CMTime.zero
    for i in 0..<count {
        try autoreleasepool {
            guard let image = CGImageSourceCreateImageAtIndex(source, i, nil) else { return }
            while !sink.isReadyForMoreMediaData {
                if writer.status == .failed { throw writerFailure(writer) }
                Thread.sleep(forTimeInterval: 0.002)
            }
            var made: CVPixelBuffer?
            if let pool = adaptor.pixelBufferPool { CVPixelBufferPoolCreatePixelBuffer(nil, pool, &made) }
            guard let buffer = made else { throw writerFailure(writer) }
            try draw(image, into: buffer)
            guard adaptor.append(buffer, withPresentationTime: time) else { throw writerFailure(writer) }
            time = time + CMTime(seconds: frameDelay(source, i), preferredTimescale: 600)
        }
        report(Double(i + 1) / Double(count))
    }
    sink.markAsFinished()
    writer.endSession(atSourceTime: time)
    try blocking { await writer.finishWriting() }
    guard writer.status == .completed else { throw writerFailure(writer) }
}

// MARK: - Re-encoding

/// One track's place in `pump`, touched only on that track's queue.
final class PumpState: @unchecked Sendable {
    var finished = false
    var reported = -1.0
}

/// How many tracks `pump` is still moving, shared between their queues.
final class Remaining: @unchecked Sendable {
    private let lock = NSLock()
    private var count: Int

    init(_ count: Int) { self.count = count }

    func done() {
        lock.lock()
        count -= 1
        lock.unlock()
    }

    var isZero: Bool {
        lock.lock()
        defer { lock.unlock() }
        return count <= 0
    }
}

/// Moves samples from each reader output into its writer input, all at
/// once (so neither track waits on the other), then finishes the file.
/// Progress follows the first pair.
func pump(_ pairs: [(AVAssetReaderOutput, AVAssetWriterInput)], reader: AVAssetReader, writer: AVAssetWriter,
          duration: CMTime, progress: @escaping (Double) -> Void) throws {
    // A counter rather than a dispatch group: a track whose writer fails
    // may never be asked for more, and a group left waiting can't be freed.
    let remaining = Remaining(pairs.count)
    let total = duration.seconds
    for (index, pair) in pairs.enumerated() {
        let (source, sink) = pair
        let state = PumpState()
        sink.requestMediaDataWhenReady(on: DispatchQueue(label: "radlet.pump.\(index)")) {
            guard !state.finished else { return }
            while sink.isReadyForMoreMediaData {
                guard let buffer = source.copyNextSampleBuffer(), sink.append(buffer) else {
                    state.finished = true
                    sink.markAsFinished()
                    remaining.done()
                    return
                }
                if index == 0, total > 0 {
                    let done = CMSampleBufferGetPresentationTimeStamp(buffer).seconds / total
                    if done - state.reported >= 0.01 {
                        progress(min(max(done, 0), 1))
                        state.reported = done
                    }
                }
            }
        }
    }
    // A writer that fails stops asking for more, so it's watched too.
    while !remaining.isZero {
        if writer.status == .failed || reader.status == .failed { break }
        Thread.sleep(forTimeInterval: 0.05)
    }
    if reader.status == .failed {
        writer.cancelWriting()
        throw Failure.failed("macOS couldn't read all of this video (\(reader.error?.localizedDescription ?? "unknown error"))")
    }
    if writer.status == .failed {
        reader.cancelReading()
        throw writerFailure(writer)
    }
    try blocking { await writer.finishWriting() }
    guard writer.status == .completed else { throw writerFailure(writer) }
}

/// Re-encodes a video track through `composition` as H.264 at `videoRate`
/// bits a second, with its sound as stereo AAC at `audioRate`.
func transcode(_ asset: AVAsset, video: AVAssetTrack, audio: AVAssetTrack?, composition: AVVideoComposition,
               to output: URL, fileType: AVFileType, videoRate: Int, audioRate: Int, frameRate: Float,
               duration: CMTime, progress: @escaping (Double) -> Void) throws {
    let reader = try AVAssetReader(asset: asset)
    let videoOut = AVAssetReaderVideoCompositionOutput(videoTracks: [video], videoSettings: [
        kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
    ])
    videoOut.videoComposition = composition
    videoOut.alwaysCopiesSampleData = false
    guard reader.canAdd(videoOut) else { throw Failure.failed("macOS can't read this video's picture") }
    reader.add(videoOut)

    let writer = try AVAssetWriter(outputURL: output, fileType: fileType)
    writer.shouldOptimizeForNetworkUse = true
    let fps = frameRate.isFinite && frameRate > 1 ? Double(frameRate) : 30
    let compression: [String: Any] = [
        AVVideoAverageBitRateKey: videoRate,
        AVVideoExpectedSourceFrameRateKey: fps,
        AVVideoMaxKeyFrameIntervalKey: Int(fps * 4),
        AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
    ]
    let videoIn = AVAssetWriterInput(mediaType: .video, outputSettings: [
        AVVideoCodecKey: AVVideoCodecType.h264,
        AVVideoWidthKey: Int(composition.renderSize.width),
        AVVideoHeightKey: Int(composition.renderSize.height),
        AVVideoCompressionPropertiesKey: compression,
    ])
    videoIn.expectsMediaDataInRealTime = false
    guard writer.canAdd(videoIn) else { throw Failure.failed("macOS can't write this video as H.264") }
    writer.add(videoIn)
    var pairs: [(AVAssetReaderOutput, AVAssetWriterInput)] = [(videoOut, videoIn)]

    if let audio {
        var layout = AudioChannelLayout()
        layout.mChannelLayoutTag = kAudioChannelLayoutTag_Stereo
        let layoutData = Data(bytes: &layout, count: MemoryLayout<AudioChannelLayout>.size)
        // Decoded to 44.1 kHz stereo (surround mixed down), then AAC.
        let audioOut = AVAssetReaderTrackOutput(track: audio, outputSettings: [
            AVFormatIDKey: kAudioFormatLinearPCM,
            AVSampleRateKey: 44_100,
            AVNumberOfChannelsKey: 2,
            AVChannelLayoutKey: layoutData,
            AVLinearPCMBitDepthKey: 16,
            AVLinearPCMIsFloatKey: false,
            AVLinearPCMIsBigEndianKey: false,
            AVLinearPCMIsNonInterleaved: false,
        ])
        audioOut.alwaysCopiesSampleData = false
        let audioIn = AVAssetWriterInput(mediaType: .audio, outputSettings: [
            AVFormatIDKey: kAudioFormatMPEG4AAC,
            AVSampleRateKey: 44_100,
            AVNumberOfChannelsKey: 2,
            AVChannelLayoutKey: layoutData,
            AVEncoderBitRateKey: audioRate,
        ])
        audioIn.expectsMediaDataInRealTime = false
        if reader.canAdd(audioOut), writer.canAdd(audioIn) {
            reader.add(audioOut)
            writer.add(audioIn)
            pairs.append((audioOut, audioIn))
        }
    }

    guard reader.startReading() else {
        throw Failure.failed("macOS can't read this video (\(reader.error?.localizedDescription ?? "unknown error"))")
    }
    guard writer.startWriting() else { throw writerFailure(writer) }
    writer.startSession(atSourceTime: .zero)
    try pump(pairs, reader: reader, writer: writer, duration: duration, progress: progress)
}

/// Re-encodes a video as H.264 and AAC sized to about `bytes`: the bitrate
/// comes from its length, and the picture shrinks as the bitrate falls. A
/// try that comes out too large is redone at a lower bitrate.
func compressToSize(_ asset: AVAsset, video: AVAssetTrack, audio: AVAssetTrack?, input: URL, to output: URL,
                    fileType: AVFileType, bytes: Int64, duration: CMTime) throws {
    if fileSize(input) <= bytes { throw Failure.nothingFound("it's already smaller than that") }
    let seconds = duration.seconds
    guard seconds.isFinite, seconds > 0 else { throw Failure.failed("couldn't read how long this video is") }
    let unwatchable = Failure.nothingFound(String(
        format: "a %.0f-second video can't be made that small and still be watchable", seconds))
    // The whole file's bits a second, less 5% for the container.
    let total = Double(bytes) * 8 * 0.95 / seconds
    let audioRate = audio == nil ? 0 : (total > 1_000_000 ? 128_000 : 64_000)
    var videoRate = total - Double(audioRate)
    guard videoRate >= 80_000 else { throw unwatchable }
    let (natural, transform, rate) = try blocking {
        (try await video.load(.naturalSize), try await video.load(.preferredTransform),
         try await video.load(.nominalFrameRate))
    }
    let upright = uprightVideoSize(natural, transform)
    let tries = 3
    for attempt in 0..<tries {
        let longest: Double
        switch videoRate {
        case 4_000_000...: longest = 1920
        case 1_800_000...: longest = 1280
        case 800_000...: longest = 960
        case 400_000...: longest = 640
        default: longest = 480
        }
        let render = fitted(upright, longest: longest)
        let composition = AVMutableVideoComposition()
        composition.renderSize = render
        composition.frameDuration = frameDuration(rate)
        let instruction = AVMutableVideoCompositionInstruction()
        instruction.timeRange = CMTimeRange(start: .zero, duration: duration)
        let layer = AVMutableVideoCompositionLayerInstruction(assetTrack: video)
        layer.setTransform(fitTransform(natural, transform, into: render), at: .zero)
        instruction.layerInstructions = [layer]
        composition.instructions = [instruction]
        try? FileManager.default.removeItem(at: output)
        // The first try takes most of the bar; any retry the rest.
        let span = attempt == 0 ? (0.0, 0.85) : (0.85, 1.0)
        try transcode(asset, video: video, audio: audio, composition: composition, to: output, fileType: fileType,
                      videoRate: Int(videoRate), audioRate: audioRate, frameRate: rate, duration: duration) {
            report(span.0 + (span.1 - span.0) * $0)
        }
        let written = fileSize(output)
        if written <= bytes { return }
        guard attempt + 1 < tries else { break }
        videoRate *= Double(bytes) / Double(max(written, 1)) * 0.92
        guard videoRate >= 80_000 else { break }
    }
    try? FileManager.default.removeItem(at: output)
    throw unwatchable
}

// MARK: - Joining

/// Clips one after another in one file. Videos take the first clip's
/// shape; the others are fitted into it, centred. As M4A, only the sound.
func join(_ args: [String]) throws {
    let output = URL(fileURLWithPath: args[0])
    let inputs = args.dropFirst(2).map { URL(fileURLWithPath: $0) }
    let fileType: AVFileType
    switch args[1].lowercased() {
    case "mp4": fileType = .mp4
    case "mov": fileType = .mov
    case "m4a": fileType = .m4a
    default: throw Failure.failed("macOS can't join clips into \(args[1].uppercased()) without FFmpeg")
    }
    guard inputs.count >= 2 else { throw Failure.failed("choose two or more clips to join") }
    let soundOnly = fileType == .m4a
    let composition = AVMutableComposition()
    guard let soundTrack = composition.addMutableTrack(withMediaType: .audio,
                                                       preferredTrackID: kCMPersistentTrackID_Invalid) else {
        throw Failure.failed("couldn't start joining")
    }
    let pictureTrack = soundOnly
        ? nil : composition.addMutableTrack(withMediaType: .video, preferredTrackID: kCMPersistentTrackID_Invalid)
    if !soundOnly && pictureTrack == nil { throw Failure.failed("couldn't start joining") }

    struct Piece {
        let start: CMTime
        let natural: CGSize
        let transform: CGAffineTransform
    }
    var pieces: [Piece] = []
    var cursor = CMTime.zero
    var frameRate: Float = 0
    var sound = false
    for url in inputs {
        let asset = AVURLAsset(url: url)
        let name = url.lastPathComponent
        let (duration, videos, audios) = try blocking {
            (try await asset.load(.duration), try await asset.loadTracks(withMediaType: .video),
             try await asset.loadTracks(withMediaType: .audio))
        }
        guard duration.seconds.isFinite, duration.seconds > 0, !(videos.isEmpty && audios.isEmpty) else {
            throw Failure.failed("macOS can't read \(name). Install FFmpeg (brew install ffmpeg) for more formats.")
        }
        if let pictureTrack {
            guard let v = videos.first else {
                throw Failure.failed("\(name) has no picture. Join videos with videos, or sound with sound.")
            }
            let (range, natural, transform, rate) = try blocking {
                (try await v.load(.timeRange), try await v.load(.naturalSize),
                 try await v.load(.preferredTransform), try await v.load(.nominalFrameRate))
            }
            try pictureTrack.insertTimeRange(range, of: v, at: cursor + range.start)
            pieces.append(Piece(start: cursor, natural: natural, transform: transform))
            frameRate = max(frameRate, rate)
        }
        if let a = audios.first {
            let range = try blocking { try await a.load(.timeRange) }
            try soundTrack.insertTimeRange(range, of: a, at: cursor + range.start)
            sound = true
        }
        cursor = cursor + duration
    }
    if !sound {
        if soundOnly { throw Failure.nothingFound("these files have no sound") }
        composition.removeTrack(soundTrack)
    }
    try? FileManager.default.removeItem(at: output)
    if soundOnly {
        try exportAsset(composition, preset: AVAssetExportPresetAppleM4A, to: output, fileType: fileType, range: nil)
        return
    }
    guard let first = pieces.first, let pictureTrack else { throw Failure.failed("there's no picture to join") }
    let render = fitted(uprightVideoSize(first.natural, first.transform), longest: 3840)
    let video = AVMutableVideoComposition()
    video.renderSize = render
    video.frameDuration = frameDuration(frameRate)
    let instruction = AVMutableVideoCompositionInstruction()
    instruction.timeRange = CMTimeRange(start: .zero, duration: composition.duration)
    let layer = AVMutableVideoCompositionLayerInstruction(assetTrack: pictureTrack)
    for piece in pieces {
        layer.setTransform(fitTransform(piece.natural, piece.transform, into: render), at: piece.start)
    }
    instruction.layerInstructions = [layer]
    video.instructions = [instruction]
    guard let session = AVAssetExportSession(asset: composition, presetName: AVAssetExportPresetHighestQuality),
          session.supportedFileTypes.contains(fileType) else {
        throw Failure.failed("macOS can't join these clips into \(fileType.rawValue)")
    }
    session.videoComposition = video
    try runExport(session, to: output, fileType: fileType, range: nil)
}

// MARK: - Burned-in subtitles

struct Cue {
    let start: Double
    let end: Double
    let text: String
}

/// Subtitle lines drawn as white text with a dark outline, centred near
/// the bottom of the picture. Each line is drawn once and reused.
final class Captions: @unchecked Sendable {
    private let cues: [Cue]
    private let lock = NSLock()
    private var cached: (index: Int, frame: CGRect, image: CIImage?)?

    init(_ cues: [Cue]) { self.cues = cues }

    /// The caption showing at `t` seconds, placed on a frame, if any.
    func image(at t: Double, frame: CGRect) -> CIImage? {
        // The latest line to start that hasn't ended.
        guard let index = cues.indices.last(where: { cues[$0].start <= t && t < cues[$0].end }) else { return nil }
        lock.lock()
        defer { lock.unlock() }
        if let c = cached, c.index == index, c.frame.equalTo(frame) { return c.image }
        let image = render(cues[index].text, frame: frame)
        cached = (index, frame, image)
        return image
    }

    private func render(_ text: String, frame: CGRect) -> CIImage? {
        guard frame.width > 0, frame.height > 0, !frame.isInfinite else { return nil }
        let size = max(14, min(frame.height, frame.width * 1.2) * 0.055)
        let font = CTFontCreateWithName("Helvetica-Bold" as CFString, size, nil)
        let fontKey = NSAttributedString.Key(kCTFontAttributeName as String)
        let fill: [NSAttributedString.Key: Any] = [
            fontKey: font,
            NSAttributedString.Key(kCTForegroundColorAttributeName as String): CGColor(gray: 1, alpha: 1),
        ]
        // A positive stroke width draws only the outline; the fill goes on top.
        let outline: [NSAttributedString.Key: Any] = [
            fontKey: font,
            NSAttributedString.Key(kCTStrokeWidthAttributeName as String): 14.0,
            NSAttributedString.Key(kCTStrokeColorAttributeName as String): CGColor(gray: 0, alpha: 0.9),
        ]
        let maxWidth = Double(frame.width) * 0.86
        var lines: [(fill: CTLine, outline: CTLine, width: CGFloat)] = []
        for paragraph in text.components(separatedBy: .newlines) {
            let trimmed = paragraph.trimmingCharacters(in: .whitespaces)
            guard !trimmed.isEmpty else { continue }
            let filled = NSAttributedString(string: trimmed, attributes: fill)
            let outlined = NSAttributedString(string: trimmed, attributes: outline)
            let setter = CTTypesetterCreateWithAttributedString(filled as CFAttributedString)
            let outlineSetter = CTTypesetterCreateWithAttributedString(outlined as CFAttributedString)
            var start = 0
            while start < filled.length {
                let count = CTTypesetterSuggestLineBreak(setter, start, maxWidth)
                guard count > 0 else { break }
                let range = CFRange(location: start, length: count)
                let line = CTTypesetterCreateLine(setter, range)
                let width = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil) - CTLineGetTrailingWhitespaceWidth(line))
                lines.append((line, CTTypesetterCreateLine(outlineSetter, range), width))
                start += count
            }
        }
        guard !lines.isEmpty else { return nil }
        let ascent = CTFontGetAscent(font), descent = CTFontGetDescent(font)
        let lineHeight = (ascent + descent) * 1.15
        let pad = ceil(size * 0.3)
        let w = Int(ceil((lines.map { $0.width }.max() ?? 0) + pad * 2))
        let h = Int(ceil(lineHeight * CGFloat(lines.count) + pad * 2))
        guard w > 0, h > 0, let space = CGColorSpace(name: CGColorSpace.sRGB),
              let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0, space: space,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        ctx.setShouldAntialias(true)
        for (k, line) in lines.enumerated() {
            let baseline = CGFloat(h) - pad - ascent - CGFloat(k) * lineHeight
            let x = (CGFloat(w) - line.width) / 2
            ctx.textPosition = CGPoint(x: x, y: baseline)
            CTLineDraw(line.outline, ctx)
            ctx.textPosition = CGPoint(x: x, y: baseline)
            CTLineDraw(line.fill, ctx)
        }
        guard let picture = ctx.makeImage() else { return nil }
        let x = frame.minX + (frame.width - CGFloat(w)) / 2
        let y = frame.minY + frame.height * 0.06
        return CIImage(cgImage: picture).transformed(by: CGAffineTransform(translationX: x.rounded(), y: y.rounded()))
    }
}

func readCues(_ url: URL) throws -> [Cue] {
    let raw = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [[String: Any]] ?? []
    return raw.compactMap { d -> Cue? in
        guard let s = (d["s"] as? NSNumber)?.doubleValue, let e = (d["e"] as? NSNumber)?.doubleValue,
              let t = d["t"] as? String, e > s,
              !t.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
        return Cue(start: s, end: e, text: t)
    }.sorted { $0.start < $1.start }
}

/// A copy of the video with the subtitles drawn into the picture, so every
/// player shows them.
func burn(_ args: [String]) throws {
    let input = URL(fileURLWithPath: args[0]), output = URL(fileURLWithPath: args[2])
    let fileType: AVFileType = args[3].lowercased() == "mov" ? .mov : .mp4
    let cues = try readCues(URL(fileURLWithPath: args[1]))
    guard !cues.isEmpty else { throw Failure.nothingFound("the subtitles have no lines") }
    let asset = AVURLAsset(url: input)
    let videos = try blocking { try await asset.loadTracks(withMediaType: .video) }
    guard !videos.isEmpty else { throw Failure.failed("there's no picture to put the subtitles on") }
    let captions = Captions(cues)
    let composition = AVMutableVideoComposition(asset: asset, applyingCIFiltersWithHandler: { request in
        let frame = request.sourceImage
        if let caption = captions.image(at: request.compositionTime.seconds, frame: frame.extent) {
            request.finish(with: caption.composited(over: frame).cropped(to: frame.extent), context: nil)
        } else {
            request.finish(with: frame, context: nil)
        }
    })
    try? FileManager.default.removeItem(at: output)
    guard let session = AVAssetExportSession(asset: asset, presetName: AVAssetExportPresetHighestQuality),
          session.supportedFileTypes.contains(fileType) else {
        throw Failure.failed("macOS can't write this video as \(fileType.rawValue)")
    }
    session.videoComposition = composition
    try runExport(session, to: output, fileType: fileType, range: nil)
}

// MARK: - Self-test

/// One line for the self-test: a three-frame GIF made into video, joined
/// to itself, with a subtitle burned in.
func videoSelftest(in dir: URL) throws -> String {
    let gif = dir.appendingPathComponent("moving.gif")
    guard let dest = CGImageDestinationCreateWithURL(gif as CFURL, UTType.gif.identifier as CFString, 3, nil),
          let space = CGColorSpace(name: CGColorSpace.sRGB) else {
        throw Failure.failed("couldn't make the test GIF")
    }
    CGImageDestinationSetProperties(dest, [kCGImagePropertyGIFDictionary: [kCGImagePropertyGIFLoopCount: 0]] as CFDictionary)
    for i in 0..<3 {
        guard let ctx = CGContext(data: nil, width: 64, height: 48, bitsPerComponent: 8, bytesPerRow: 0, space: space,
                                  bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else {
            throw Failure.failed("couldn't make the test GIF")
        }
        ctx.setFillColor(CGColor(red: CGFloat(i) / 2, green: 0.3, blue: 1 - CGFloat(i) / 2, alpha: 1))
        ctx.fill(CGRect(x: 0, y: 0, width: 64, height: 48))
        guard let frame = ctx.makeImage() else { throw Failure.failed("couldn't make the test GIF") }
        CGImageDestinationAddImage(dest, frame,
                                   [kCGImagePropertyGIFDictionary: [kCGImagePropertyGIFDelayTime: 0.2]] as CFDictionary)
    }
    guard CGImageDestinationFinalize(dest) else { throw Failure.failed("couldn't make the test GIF") }
    let length = { (url: URL) in try blocking { try await AVURLAsset(url: url).load(.duration) }.seconds }

    let mp4 = dir.appendingPathComponent("moving.mp4")
    try media([gif.path, mp4.path, "mp4", "frames"])
    let made = try length(mp4)
    guard abs(made - 0.6) < 0.05 else {
        throw Failure.failed(String(format: "the test GIF became %.2f s of video, not 0.6 s", made))
    }
    let joined = dir.appendingPathComponent("joined.mp4")
    try join([joined.path, "mp4", mp4.path, mp4.path])
    let both = try length(joined)
    guard abs(both - 1.2) < 0.1 else {
        throw Failure.failed(String(format: "the joined test video is %.2f s long, not 1.2 s", both))
    }
    let cues = dir.appendingPathComponent("cues.json")
    try JSONSerialization.data(withJSONObject: [["s": 0.0, "e": 1.0, "t": "Radlet"]]).write(to: cues)
    let burned = dir.appendingPathComponent("burned.mp4")
    try burn([joined.path, cues.path, burned.path, "mp4"])
    guard fileSize(burned) > 0 else { throw Failure.failed("burning in the test subtitle made nothing") }
    return "video: ok (GIF → MP4, joined, subtitle burned in)"
}
