import AppKit
import AVFoundation
import CoreImage
import CoreImage.CIFilterBuiltins
import ImageIO
import SwiftUI
import UniformTypeIdentifiers
import RadletKit

/// Opens the option windows for tools such as Crop and Compress. A window
/// builds the final action from your choices, then hands the slot back to
/// run as an ordinary job (HUD, Undo and all).
@MainActor
final class ToolWindowController: NSObject, NSWindowDelegate {
    var onApply: ((Slot, [URL]) -> Void)?
    /// Renames happen in the window itself; this records them.
    var onRenamed: (([HistoryEntry.Rename]) -> Void)?
    private var window: NSWindow?
    private var stop: (() -> Void)?

    func open(_ slot: Slot, files: [URL]) {
        close()
        guard let kind = slot.window, !files.isEmpty else { return }
        Log.app("tool window \(kind) for \(describeFiles(files))")
        let finish: (JSONValue?) -> Void = { [weak self] action in
            guard let self else { return }
            self.close()
            if let action { self.onApply?(slot.with(action: action), files) }
        }

        let renamed: ([HistoryEntry.Rename]) -> Void = { [weak self] renames in
            self?.close()
            if !renames.isEmpty { self?.onRenamed?(renames) }
        }
        guard let tool = Self.build(kind, files: files, finish: finish, renamed: renamed) else {
            // A newer engine may offer a window this app doesn't know yet:
            // run the slot's default action rather than doing nothing.
            Log.app("no tool window called \(kind); running its default")
            onApply?(slot.with(action: slot.action), files)
            return
        }

        let host = NSHostingController(rootView: tool.view)
        let w = NSWindow(contentViewController: host)
        let what = files.count == 1 ? files[0].lastPathComponent : "\(files.count) files"
        w.title = "\(slot.plainLabel) — \(what)"
        w.styleMask = tool.resizable ? [.titled, .closable, .miniaturizable, .resizable] : [.titled, .closable]
        w.isReleasedWhenClosed = false
        w.delegate = self
        w.center()
        window = w
        stop = tool.stop
        NSApp.activate()
        w.makeKeyAndOrderFront(nil)
    }

    private struct Tool {
        let view: AnyView
        let resizable: Bool
        let stop: () -> Void
    }

    private static func build(_ kind: String, files: [URL], finish: @escaping (JSONValue?) -> Void,
                              renamed: @escaping ([HistoryEntry.Rename]) -> Void) -> Tool? {
        switch kind {
        case "resize":
            let m = ResizeModel(files: files, finish: finish)
            return Tool(view: AnyView(ResizeView(model: m)), resizable: false, stop: { m.stop() })
        case "crop":
            let m = CropModel(files: files, finish: finish)
            return Tool(view: AnyView(CropView(model: m)), resizable: true, stop: { m.stop() })
        case "redact":
            let m = RedactModel(files: files, finish: finish)
            return Tool(view: AnyView(RedactView(model: m)), resizable: true, stop: { m.stop() })
        case "compress":
            let m = CompressModel(files: files, finish: finish)
            return Tool(view: AnyView(CompressView(model: m)), resizable: true, stop: { m.stop() })
        case "split_pdf":
            let m = SplitModel(files: files, finish: finish)
            return Tool(view: AnyView(SplitView(model: m)), resizable: false, stop: {})
        case "pages_pdf":
            let m = PagesModel(file: files[0], finish: finish)
            return Tool(view: AnyView(PagesView(model: m)), resizable: true, stop: {})
        case "page_numbers":
            let m = PageNumbersModel(files: files, finish: finish)
            return Tool(view: AnyView(PageNumbersView(model: m)), resizable: true, stop: { m.stop() })
        case "watermark":
            let m = WatermarkModel(files: files, finish: finish)
            return Tool(view: AnyView(WatermarkView(model: m)), resizable: true, stop: { m.stop() })
        case "pdf_password", "zip_password", "unzip_password":
            let purpose: PasswordModel.Purpose = kind == "zip_password" ? .zip : kind == "unzip_password" ? .unzip : .pdf
            let m = PasswordModel(files: files, purpose: purpose, finish: finish)
            return Tool(view: AnyView(PasswordView(model: m)), resizable: false, stop: {})
        case "redact_text":
            let m = RedactTextModel(files: files, finish: finish)
            return Tool(view: AnyView(RedactTextView(model: m)), resizable: false, stop: {})
        case "trim", "make_gif":
            let m = TrimModel(files: files, gif: kind == "make_gif", finish: finish)
            return Tool(view: AnyView(TrimView(model: m)), resizable: true, stop: { m.stop() })
        case "rename":
            let m = RenameModel(files: files, done: renamed)
            return Tool(view: AnyView(RenameView(model: m)), resizable: true, stop: {})
        case "privacy":
            let m = PrivacyModel(files: files, finish: finish)
            return Tool(view: AnyView(PrivacyView(model: m)), resizable: true, stop: {})
        case "subtitles":
            let m = SubtitlesModel(files: files, finish: finish)
            return Tool(view: AnyView(SubtitlesView(model: m)), resizable: false, stop: { m.stop() })
        case "translate":
            let m = TranslateModel(files: files, finish: finish)
            return Tool(view: AnyView(TranslateView(model: m)), resizable: false, stop: {})
        default:
            return nil
        }
    }

    func close() {
        window?.close()  // windowWillClose cleans up
    }

    func windowWillClose(_ notification: Notification) {
        guard let closing = notification.object as? NSWindow, closing === window else { return }
        stop?()
        stop = nil
        window = nil
    }
}

// MARK: - Pictures

/// An upright preview of an image or video frame.
struct Picture {
    let image: CGImage
    /// The original's upright size in pixels.
    let pixelSize: CGSize
    /// False when the preview came from a conversion and the original's
    /// pixel size isn't known (its proportions still are).
    let sizeKnown: Bool
    let isVideo: Bool

    var previewSize: CGSize { CGSize(width: image.width, height: image.height) }
}

enum PictureLoader {
    static let maxPixels = 2000

    /// Images through ImageIO, videos through AVFoundation, and anything
    /// else the engine can turn into a PNG.
    @MainActor
    static func load(_ url: URL, scratch: ScratchFolder) async -> Picture? {
        if let p = image(url) { return p }
        if let p = await videoFrame(url) { return p }
        // Formats ImageIO can't read, such as QOI: ask the engine.
        let action: JSONValue = .object([
            "kind": .string("steps"),
            "steps": .array([
                .object(["op": .string("resize"), "max": .int(Int64(maxPixels))]),
                .object(["op": .string("convert"), "to": .string("png")]),
            ]),
        ])
        guard let folder = scratch.fresh() else { return nil }
        let outcome = try? await PreviewRun().run(file: url, action: action, folder: folder)
        scratch.tidy()
        guard let outcome, outcome.status == "done", let made = outcome.createdItems.first,
              let p = image(URL(fileURLWithPath: made)) else { return nil }
        return Picture(image: p.image, pixelSize: p.previewSize, sizeKnown: false, isVideo: false)
    }

    static func image(_ url: URL) -> Picture? {
        guard let src = CGImageSourceCreateWithURL(url as CFURL, [kCGImageSourceShouldCache: false] as CFDictionary),
              CGImageSourceGetCount(src) > 0 else { return nil }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: maxPixels,
        ]
        guard let thumb = CGImageSourceCreateThumbnailAtIndex(src, 0, options as CFDictionary) else { return nil }
        if let props = CGImageSourceCopyPropertiesAtIndex(src, 0, nil) as? [CFString: Any],
           let w = props[kCGImagePropertyPixelWidth] as? Int, let h = props[kCGImagePropertyPixelHeight] as? Int,
           w > 0, h > 0 {
            let orientation = props[kCGImagePropertyOrientation] as? Int ?? 1
            let size = (5...8).contains(orientation) ? CGSize(width: h, height: w) : CGSize(width: w, height: h)
            return Picture(image: thumb, pixelSize: size, sizeKnown: true, isVideo: false)
        }
        return Picture(image: thumb, pixelSize: CGSize(width: thumb.width, height: thumb.height),
                       sizeKnown: false, isVideo: false)
    }

    static func videoFrame(_ url: URL) async -> Picture? {
        let asset = AVURLAsset(url: url)
        guard let track = try? await asset.loadTracks(withMediaType: .video).first,
              let geometry = try? await track.load(.naturalSize, .preferredTransform) else { return nil }
        let upright = CGRect(origin: .zero, size: geometry.0).applying(geometry.1)
        let size = CGSize(width: abs(upright.width).rounded(), height: abs(upright.height).rounded())
        guard size.width > 0, size.height > 0 else { return nil }
        let generator = AVAssetImageGenerator(asset: asset)
        generator.appliesPreferredTrackTransform = true
        generator.maximumSize = CGSize(width: maxPixels, height: maxPixels)
        let seconds = (try? await asset.load(.duration)).map(CMTimeGetSeconds) ?? 0
        let at = CMTime(seconds: seconds.isFinite ? min(1, seconds / 2) : 0, preferredTimescale: 600)
        guard let frame = try? await generator.image(at: at) else { return nil }
        return Picture(image: frame.image, pixelSize: size, sizeKnown: true, isVideo: true)
    }

    /// Video files, including formats macOS has no type for (MKV, WebM…).
    static func isVideo(_ url: URL) -> Bool {
        let ext = url.pathExtension.lowercased()
        if ["mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "flv"].contains(ext) { return true }
        guard let type = UTType(filenameExtension: ext) else { return false }
        return type.conforms(to: .movie) || type.conforms(to: .video)
    }

    /// The preview with each box pixelated (and blurred) the way the engine
    /// does it, block sizes included, so the boxes show the real result.
    static func redacted(_ p: Picture, boxes: [NormRect], blur: Bool) -> CGImage? {
        guard !boxes.isEmpty else { return nil }
        let base = CIImage(cgImage: p.image)
        let w = CGFloat(p.image.width), h = CGFloat(p.image.height)
        // Preview pixels per original pixel.
        let scale = p.pixelSize.width > 0 ? w / p.pixelSize.width : 1
        var out = base
        for box in boxes {
            let short = min(CGFloat(box.w) * p.pixelSize.width, CGFloat(box.h) * p.pixelSize.height)
            let block = (blur ? max(8, (short / 16).rounded(.down)) : max(12, (short / 10).rounded(.down))) * scale
            // Core Image's origin is the bottom left.
            let rect = CGRect(x: CGFloat(box.x) * w, y: (1 - CGFloat(box.y) - CGFloat(box.h)) * h,
                              width: CGFloat(box.w) * w, height: CGFloat(box.h) * h).integral
            let region = base.cropped(to: rect).clampedToExtent()
            let pixellate = CIFilter.pixellate()
            pixellate.inputImage = region
            pixellate.scale = Float(max(block, 1))
            pixellate.center = CGPoint(x: rect.minX, y: rect.maxY)  // blocks start at the top left
            var covered = pixellate.outputImage ?? region
            if blur {
                let gaussian = CIFilter.gaussianBlur()
                gaussian.inputImage = covered.cropped(to: rect).clampedToExtent()
                gaussian.radius = Float(min(max(short / 8, 6), 60) * scale)
                covered = gaussian.outputImage ?? covered
            }
            out = covered.cropped(to: rect).composited(over: out)
        }
        return ciContext.createCGImage(out, from: base.extent)
    }

    private static let ciContext = CIContext()
}

/// A private temporary folder for previews, removed when the window closes.
@MainActor
final class ScratchFolder {
    private static let prefix = "Radlet Preview "
    let url = FileManager.default.temporaryDirectory
        .appendingPathComponent(ScratchFolder.prefix + UUID().uuidString, isDirectory: true)
    private(set) var isRemoved = false

    /// A new empty subfolder, or nil once the window has closed.
    func fresh() -> URL? {
        guard !isRemoved else { return nil }
        let dir = url.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    func remove() {
        isRemoved = true
        try? FileManager.default.removeItem(at: url)
    }

    /// Call after a preview job ends: a job still running when the window
    /// closed may have recreated the folder.
    func tidy() {
        if isRemoved { try? FileManager.default.removeItem(at: url) }
    }

    /// Deletes preview folders left by a previous run (a quit or crash
    /// with a tool window open).
    static func removeLeftovers() {
        let tmp = FileManager.default.temporaryDirectory
        let names = (try? FileManager.default.contentsOfDirectory(atPath: tmp.path)) ?? []
        for name in names where name.hasPrefix(prefix) {
            try? FileManager.default.removeItem(at: tmp.appendingPathComponent(name))
        }
    }
}

/// Runs one action on one file into a folder, outside the HUD, and returns
/// its outcome (nil if it was cancelled).
@MainActor
final class PreviewRun {
    private var job: Int?
    private(set) var cancelled = false
    private var outcome: Outcome?
    private var continuation: CheckedContinuation<Outcome?, Error>?

    func run(file: URL, action: JSONValue, folder: URL) async throws -> Outcome? {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Outcome?, Error>) in
            continuation = cont
            let output: JSONValue = .object(["kind": .string("directory"), "path": .string(folder.path)])
            Task { [self] in
                do {
                    let job = try await EngineClient.shared.run(
                        files: [file], action: action, output: output, isolate: true
                    ) { [self] event in
                        switch event {
                        case .finished(let o): outcome = o
                        case .jobFinished: finish(.success(cancelled ? nil : outcome))
                        default: break
                        }
                    }
                    self.job = job
                    if cancelled { EngineClient.shared.cancel(job: job) }
                } catch {
                    finish(.failure(error))
                }
            }
        }
    }

    func cancel() {
        cancelled = true
        if let job { EngineClient.shared.cancel(job: job) }
    }

    private func finish(_ result: Result<Outcome?, Error>) {
        continuation?.resume(with: result)
        continuation = nil
    }
}

// MARK: - Shared views

/// Cancel and the main button, with a note on the left.
struct ToolFooter: View {
    let note: String
    let applyTitle: String
    let canApply: Bool
    let cancel: () -> Void
    let apply: () -> Void
    /// Offers "Save as Loadout…" when set.
    var saveRecipe: (() -> Void)? = nil

    var body: some View {
        HStack(spacing: 10) {
            Text(note)
                .font(.callout)
                .foregroundStyle(.secondary)
                .lineLimit(2)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 12)
            if let saveRecipe {
                Button("Save as Loadout…", action: saveRecipe)
                    .disabled(!canApply)
            }
            Button("Cancel", action: cancel)
                .keyboardShortcut(.cancelAction)
            Button(applyTitle, action: apply)
                .keyboardShortcut(.defaultAction)
                .disabled(!canApply)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
    }
}

/// A picture fitted into the available space, with an overlay drawn in the
/// same coordinates and an optional drag handler that receives positions
/// as fractions of the picture.
struct PictureCanvas<Overlay: View>: View {
    let picture: Picture?
    let loading: Bool
    var onDrag: ((_ start: CGPoint, _ current: CGPoint) -> Void)?
    var onEnd: (() -> Void)?
    let overlay: (CGRect) -> Overlay

    var body: some View {
        GeometryReader { geo in
            let content = picture?.previewSize ?? CGSize(width: 16, height: 10)
            let frame = ImageFit.frame(content: content, in: geo.size, padding: 16)
            ZStack(alignment: .topLeading) {
                Color(nsColor: .underPageBackgroundColor)
                if let picture {
                    Image(decorative: picture.image, scale: 1)
                        .resizable()
                        .interpolation(.high)
                        .frame(width: frame.width, height: frame.height)
                        .position(x: frame.midX, y: frame.midY)
                    overlay(frame)
                } else {
                    VStack(spacing: 8) {
                        if loading {
                            ProgressView()
                        } else {
                            Image(systemName: "eye.slash").font(.system(size: 28))
                            Text("No preview for this file").font(.callout)
                        }
                    }
                    .foregroundStyle(.secondary)
                    .frame(width: geo.size.width, height: geo.size.height)
                }
            }
            .frame(width: geo.size.width, height: geo.size.height)
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 0, coordinateSpace: .local)
                    .onChanged { v in
                        onDrag?(ImageFit.fraction(of: v.startLocation, in: frame),
                                ImageFit.fraction(of: v.location, in: frame))
                    }
                    .onEnded { _ in onEnd?() },
                including: onDrag == nil ? .subviews : .all
            )
        }
    }
}

/// Small preview for windows that don't edit the picture.
struct Thumbnail: View {
    let picture: Picture?
    let loading: Bool

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 8).fill(Color.primary.opacity(0.05))
            if let picture {
                Image(decorative: picture.image, scale: 1)
                    .resizable()
                    .interpolation(.high)
                    .aspectRatio(contentMode: .fit)
                    .padding(6)
            } else if loading {
                ProgressView().controlSize(.small)
            } else {
                Image(systemName: "photo").font(.system(size: 26)).foregroundStyle(.tertiary)
            }
        }
    }
}

enum Bytes {
    static func text(_ n: Int64) -> String {
        ByteCountFormatter.string(fromByteCount: n, countStyle: .file)
    }

    static func size(of url: URL) -> Int64 {
        let values = try? url.resourceValues(forKeys: [.fileSizeKey])
        return Int64(values?.fileSize ?? 0)
    }
}
