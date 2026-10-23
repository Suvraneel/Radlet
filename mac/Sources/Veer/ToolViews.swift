import AppKit
import PDFKit
import SwiftUI
import VeerKit

// Each tool window is a model (an ObservableObject, never `@State`: see
// LoginItem) and a view. A model calls `finish` with the action to run, or
// nil when cancelled.

// MARK: - Resize

@MainActor
final class ResizeModel: ObservableObject {
    @Published var mode: ToolAction.ResizeMode = .percent {
        didSet { if mode != oldValue { text = String(defaultValue(for: mode)) } }
    }
    /// Kept as text so the value is current on every keystroke, even when
    /// Return applies straight from the field.
    @Published var text = "50"
    @Published private(set) var picture: Picture?
    @Published private(set) var loading = true
    let files: [URL]
    /// Every file is an SVG: drawn afresh at any size, so it can grow.
    let vector: Bool
    private let finish: (JSONValue?) -> Void
    private let scratch = ScratchFolder()

    init(files: [URL], finish: @escaping (JSONValue?) -> Void) {
        self.files = files
        self.finish = finish
        vector = files.allSatisfy { ["svg", "svgz"].contains($0.pathExtension.lowercased()) }
        if vector { text = "200" }
        Task { [weak self] in await self?.load() }
    }

    private func load() async {
        let p = await PictureLoader.load(files[0], scratch: scratch)
        guard !scratch.isRemoved else { return }  // the window closed meanwhile
        picture = p
        loading = false
    }

    private var known: CGSize? {
        guard let p = picture, p.sizeKnown else { return nil }
        return p.pixelSize
    }

    private func defaultValue(for mode: ToolAction.ResizeMode) -> Int {
        switch mode {
        case .percent: return vector ? 200 : 50
        case .max where vector: return 1024
        case .max:
            guard let s = known else { return 1920 }
            let longest = Int(max(s.width, s.height))
            return longest > 1920 ? 1920 : max(1, longest / 2)
        case .width: return known.map { max(1, Int($0.width) / 2) } ?? 1280
        case .height: return known.map { max(1, Int($0.height) / 2) } ?? 720
        }
    }

    var value: Int? { Int(text.trimmingCharacters(in: .whitespaces)) }
    var range: ClosedRange<Int> { mode == .percent ? (vector ? 1...1000 : 1...100) : 1...16_384 }
    var unit: String { mode == .percent ? "%" : "px" }

    var presets: [Int] {
        switch mode {
        case .percent: return vector ? [100, 200, 400] : [25, 50, 75]
        case .max where vector: return [512, 1024, 2048, 4096]
        case .max: return [1080, 1920, 2560, 3840]
        case .width: return [640, 1280, 1920]
        case .height: return [480, 720, 1080]
        }
    }

    func presetTitle(_ v: Int) -> String {
        mode == .percent ? "\(v)%" : String(v)
    }

    func nudge(_ direction: Int) {
        let step = mode == .percent ? 5 : 10
        // Clamp first: a huge typed number mustn't overflow.
        let base = min(max(value ?? defaultValue(for: mode), range.lowerBound), range.upperBound)
        text = String(min(max(base + direction * step, range.lowerBound), range.upperBound))
    }

    var outputSize: CGSize? {
        guard let s = known, let v = value, range.contains(v) else { return nil }
        return ToolAction.resizedSize(s, mode, v, vector: vector)
    }

    var problem: String? {
        guard let v = value else { return "Enter a whole number." }
        guard range.contains(v) else {
            return mode == .percent ? "Use 1 to \(range.upperBound)%." : "Use 1 to 16,384 pixels."
        }
        if let out = outputSize, max(out.width, out.height) > 16_384 { return "That would be larger than 16,384 pixels." }
        return nil
    }

    var canApply: Bool {
        guard problem == nil else { return false }
        // A drawing at 100% still becomes a PNG at its own size.
        if vector { return true }
        if mode == .percent && value == 100 { return false }
        if files.count == 1, let s = known, let out = outputSize { return out != s }
        return true
    }

    var summary: String {
        if let problem { return problem }
        guard let s = known, let out = outputSize else {
            return files.count > 1 ? "Each file keeps its proportions." : ""
        }
        let first = files.count > 1 ? "First file: " : ""
        let change = out == s ? " (no change)" : (out.width > s.width ? " (enlarged)" : "")
        return "\(first)\(Int(s.width)) × \(Int(s.height)) → \(Int(out.width)) × \(Int(out.height)) px\(change)"
    }

    var note: String {
        if vector { return "Drawn sharp at the new size, as PNG. The SVG stays as it is." }
        return files.count > 1 ? "Resizes all \(files.count) files. Each keeps its proportions." : "The original stays as it is."
    }

    func cancel() { finish(nil) }
    func apply() {
        guard canApply, let v = value else { return }
        finish(ToolAction.resize(mode, v))
    }
    func saveRecipe() {
        guard canApply, let v = value else { return }
        RecipeSaver.save(ToolAction.resize(mode, v), suggested: mode == .percent ? "Resize to \(v)%" : "Resize to \(v) px")
    }
    func stop() { scratch.remove() }
}

struct ResizeView: View {
    @ObservedObject var model: ResizeModel

    var body: some View {
        VStack(spacing: 0) {
            HStack(alignment: .top, spacing: 18) {
                Thumbnail(picture: model.picture, loading: model.loading)
                    .frame(width: 120, height: 120)
                VStack(alignment: .leading, spacing: 12) {
                    Picker("Resize by", selection: $model.mode) {
                        ForEach(ToolAction.ResizeMode.allCases) { m in Text(m.title).tag(m) }
                    }
                    .frame(width: 260)
                    HStack(spacing: 6) {
                        TextField("", text: $model.text)
                            .multilineTextAlignment(.trailing)
                            .frame(width: 80)
                        Text(model.unit).foregroundStyle(.secondary)
                        Stepper("", onIncrement: { model.nudge(1) }, onDecrement: { model.nudge(-1) })
                            .labelsHidden()
                    }
                    HStack(spacing: 6) {
                        ForEach(model.presets, id: \.self) { v in
                            Button(model.presetTitle(v)) { model.text = String(v) }
                                .controlSize(.small)
                        }
                    }
                    Text(model.summary)
                        .font(.callout.monospacedDigit())
                        .foregroundColor(model.problem == nil ? .secondary : .red)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(20)
            Divider()
            ToolFooter(note: model.note, applyTitle: "Resize", canApply: model.canApply,
                       cancel: { model.cancel() }, apply: { model.apply() },
                       saveRecipe: { model.saveRecipe() })
        }
        .frame(width: 500)
    }
}

// MARK: - Crop

enum AspectChoice: String, CaseIterable, Identifiable {
    case free, original, square, r4x3, r3x4, r3x2, r2x3, r16x9, r9x16, r4x5

    var id: String { rawValue }

    var title: String {
        switch self {
        case .free: return "Free"
        case .original: return "Original"
        case .square: return "Square"
        case .r4x3: return "4:3"
        case .r3x4: return "3:4"
        case .r3x2: return "3:2"
        case .r2x3: return "2:3"
        case .r16x9: return "16:9"
        case .r9x16: return "9:16"
        case .r4x5: return "4:5"
        }
    }

    func ratio(original: CGSize) -> Double? {
        switch self {
        case .free: return nil
        case .original: return original.height > 0 ? Double(original.width / original.height) : nil
        case .square: return 1
        case .r4x3: return 4.0 / 3.0
        case .r3x4: return 3.0 / 4.0
        case .r3x2: return 3.0 / 2.0
        case .r2x3: return 2.0 / 3.0
        case .r16x9: return 16.0 / 9.0
        case .r9x16: return 9.0 / 16.0
        case .r4x5: return 4.0 / 5.0
        }
    }
}

@MainActor
final class CropModel: ObservableObject {
    @Published var editor = CropEditor(pictureSize: .zero)
    @Published var aspect: AspectChoice = .free {
        didSet { if aspect != oldValue { editor.setAspect(aspect.ratio(original: editor.pictureSize)) } }
    }
    @Published private(set) var picture: Picture?
    @Published private(set) var loading = true
    let files: [URL]
    /// Where the current drag began. SwiftUI may never report the end of a
    /// cancelled drag, so a new start point means a new drag.
    private var dragStart: CGPoint?
    private let finish: (JSONValue?) -> Void
    private let scratch = ScratchFolder()

    init(files: [URL], finish: @escaping (JSONValue?) -> Void) {
        self.files = files
        self.finish = finish
        Task { [weak self] in await self?.load() }
    }

    private func load() async {
        let p = await PictureLoader.load(files[0], scratch: scratch)
        guard !scratch.isRemoved else { return }
        picture = p
        editor.pictureSize = p?.pixelSize ?? .zero
        if aspect != .free { editor.setAspect(aspect.ratio(original: editor.pictureSize)) }
        loading = false
    }

    func drag(from start: CGPoint, to point: CGPoint) {
        guard picture != nil else { return }
        if start != dragStart {
            if dragStart != nil { editor.end() }
            dragStart = start
            editor.begin(at: start)
        }
        editor.update(to: point)
    }

    func endDrag() {
        editor.end()
        dragStart = nil
    }

    func reset() { editor.reset() }

    var canApply: Bool { picture != nil && editor.rect.isUsable && !editor.rect.isFull }

    var sizeText: String {
        guard let p = picture, p.sizeKnown else { return "" }
        let px = editor.rect.pixels(of: p.pixelSize)
        return "\(px.width) × \(px.height) px"
    }

    var note: String {
        if picture == nil && !loading { return "Veer can't preview this file, so it can't be cropped here." }
        if files.count > 1 { return "The same area is cropped from all \(files.count) files." }
        return "Drag to choose the area to keep. Drag inside it to move it, or a corner to resize."
    }

    func cancel() { finish(nil) }
    func apply() {
        guard canApply else { return }
        finish(ToolAction.crop(editor.rect))
    }
    func saveRecipe() {
        guard canApply else { return }
        RecipeSaver.save(ToolAction.crop(editor.rect), suggested: "Crop \(aspect.title)")
    }
    func stop() { scratch.remove() }
}

struct CropView: View {
    @ObservedObject var model: CropModel

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                Picker("Aspect ratio", selection: $model.aspect) {
                    ForEach(AspectChoice.allCases) { a in Text(a.title).tag(a) }
                }
                .frame(width: 220)
                Button("Reset") { model.reset() }
                Spacer()
                Text(model.sizeText)
                    .font(.callout.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
            .padding(12)
            Divider()
            PictureCanvas(
                picture: model.picture,
                loading: model.loading,
                onDrag: { start, point in model.drag(from: start, to: point) },
                onEnd: { model.endDrag() },
                overlay: { frame in CropOverlay(box: ImageFit.viewRect(model.editor.rect, in: frame), frame: frame) }
            )
            Divider()
            ToolFooter(note: model.note, applyTitle: "Crop", canApply: model.canApply,
                       cancel: { model.cancel() }, apply: { model.apply() },
                       saveRecipe: { model.saveRecipe() })
        }
        .frame(minWidth: 560, idealWidth: 760, maxWidth: .infinity, minHeight: 440, idealHeight: 600, maxHeight: .infinity)
    }
}

/// Dims everything outside the crop box and draws its frame, thirds and
/// corner handles.
struct CropOverlay: View {
    let box: CGRect
    let frame: CGRect

    var body: some View {
        ZStack(alignment: .topLeading) {
            Path { p in
                p.addRect(frame)
                p.addRect(box)
            }
            .fill(Color.black.opacity(0.5), style: FillStyle(eoFill: true))
            Path { p in
                for i in 1...2 {
                    let x = box.minX + box.width * CGFloat(i) / 3
                    let y = box.minY + box.height * CGFloat(i) / 3
                    p.move(to: CGPoint(x: x, y: box.minY))
                    p.addLine(to: CGPoint(x: x, y: box.maxY))
                    p.move(to: CGPoint(x: box.minX, y: y))
                    p.addLine(to: CGPoint(x: box.maxX, y: y))
                }
            }
            .stroke(Color.white.opacity(0.45), lineWidth: 0.5)
            Path { p in p.addRect(box) }
                .stroke(Color.white, lineWidth: 1.5)
            Path { p in
                for corner in [CGPoint(x: box.minX, y: box.minY), CGPoint(x: box.maxX, y: box.minY),
                               CGPoint(x: box.minX, y: box.maxY), CGPoint(x: box.maxX, y: box.maxY)] {
                    p.addRect(CGRect(x: corner.x - 5, y: corner.y - 5, width: 10, height: 10))
                }
            }
            .fill(Color.white)
        }
        .allowsHitTesting(false)
    }
}

// MARK: - Redact

enum RedactChoice: String, CaseIterable, Identifiable {
    case black, pixelate, blur

    var id: String { rawValue }

    var title: String {
        switch self {
        case .black: return "Black box"
        case .pixelate: return "Pixelate"
        case .blur: return "Blur"
        }
    }
}

@MainActor
final class RedactModel: ObservableObject {
    @Published var editor = RedactEditor() {
        didSet { if editor.rects != oldValue.rects { updateCovered() } }
    }
    @Published var style: RedactChoice = .black {
        didSet { if style != oldValue { updateCovered() } }
    }
    @Published private(set) var picture: Picture?
    @Published private(set) var covered: CGImage?
    @Published private(set) var loading = true
    let files: [URL]
    /// Where the current drag began. SwiftUI may never report the end of a
    /// cancelled drag, so a new start point means a new drag.
    private var dragStart: CGPoint?
    private let finish: (JSONValue?) -> Void
    private let scratch = ScratchFolder()

    init(files: [URL], finish: @escaping (JSONValue?) -> Void) {
        self.files = files
        self.finish = finish
        hasVideo = files.contains { PictureLoader.isVideo($0) }
        Task { [weak self] in await self?.load() }
    }

    private func load() async {
        let p = await PictureLoader.load(files[0], scratch: scratch)
        guard !scratch.isRemoved else { return }
        picture = p
        if isVideo { style = .black }
        updateCovered()
        loading = false
    }

    /// Video redaction is always solid black, so with any video in the
    /// selection every file gets black boxes.
    private let hasVideo: Bool
    var isVideo: Bool { hasVideo || (picture?.isVideo ?? false) }

    private func updateCovered() {
        guard let p = picture, style != .black, !editor.rects.isEmpty else {
            covered = nil
            return
        }
        covered = PictureLoader.redacted(p, boxes: editor.rects, blur: style == .blur)
    }

    func drag(from start: CGPoint, to point: CGPoint) {
        guard picture != nil else { return }
        if start != dragStart {
            if dragStart != nil { editor.end() }
            dragStart = start
            editor.begin(at: start)
        }
        editor.update(to: point)
    }

    func endDrag() {
        editor.end()
        dragStart = nil
    }

    func removeSelected() { editor.removeSelected() }
    func clear() {
        editor.rects.removeAll()
        editor.selected = nil
    }

    var canApply: Bool { picture != nil && !editor.rects.isEmpty }

    var countText: String {
        let n = editor.rects.count
        return n == 0 ? "" : "\(n) area\(n == 1 ? "" : "s")"
    }

    var note: String {
        if picture == nil && !loading { return "Veer can't preview this file, so it can't be redacted here." }
        if style != .black { return "Pixelate and blur hide detail, but only a black box is safe for text such as passwords." }
        if files.count > 1 { return "The same areas are covered in all \(files.count) files." }
        return "Drag to cover an area. Click a box to select it, then press Delete to remove it."
    }

    func cancel() { finish(nil) }
    func apply() {
        guard canApply else { return }
        finish(ToolAction.redact(editor.rects, style: isVideo ? RedactChoice.black.rawValue : style.rawValue))
    }
    func saveRecipe() {
        guard canApply else { return }
        RecipeSaver.save(ToolAction.redact(editor.rects, style: isVideo ? RedactChoice.black.rawValue : style.rawValue),
                         suggested: "Redact")
    }
    func stop() { scratch.remove() }
}

struct RedactView: View {
    @ObservedObject var model: RedactModel

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                Picker("Style", selection: $model.style) {
                    ForEach(RedactChoice.allCases) { s in Text(s.title).tag(s) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .frame(width: 280)
                .disabled(model.isVideo)
                Button("Remove") { model.removeSelected() }
                    .keyboardShortcut(.delete, modifiers: [])
                    .disabled(model.editor.selected == nil)
                Button("Clear") { model.clear() }
                    .disabled(model.editor.rects.isEmpty)
                Spacer()
                Text(model.countText).font(.callout).foregroundStyle(.secondary)
            }
            .padding(12)
            Divider()
            PictureCanvas(
                picture: model.picture,
                loading: model.loading,
                onDrag: { start, point in model.drag(from: start, to: point) },
                onEnd: { model.endDrag() },
                overlay: { frame in
                    RedactOverlay(
                        boxes: model.editor.rects.map { ImageFit.viewRect($0, in: frame) },
                        pending: model.editor.pending.map { ImageFit.viewRect($0, in: frame) },
                        selected: model.editor.selected,
                        covered: model.covered,
                        frame: frame
                    )
                }
            )
            Divider()
            ToolFooter(note: model.note, applyTitle: "Redact", canApply: model.canApply,
                       cancel: { model.cancel() }, apply: { model.apply() },
                       saveRecipe: { model.saveRecipe() })
        }
        .frame(minWidth: 560, idealWidth: 760, maxWidth: .infinity, minHeight: 440, idealHeight: 600, maxHeight: .infinity)
    }
}

/// All redaction boxes as one shape.
struct BoxesShape: Shape {
    let boxes: [CGRect]

    func path(in rect: CGRect) -> Path {
        var p = Path()
        for box in boxes { p.addRect(box) }
        return p
    }
}

/// Draws each box as it will look: black, or the preview with the boxes
/// pixelated or blurred.
struct RedactOverlay: View {
    let boxes: [CGRect]
    let pending: CGRect?
    let selected: Int?
    let covered: CGImage?
    let frame: CGRect

    var body: some View {
        ZStack(alignment: .topLeading) {
            if let covered {
                // The whole preview with every box already covered.
                Image(decorative: covered, scale: 1)
                    .resizable()
                    .interpolation(.high)
                    .frame(width: frame.width, height: frame.height)
                    .position(x: frame.midX, y: frame.midY)
            } else {
                BoxesShape(boxes: boxes).fill(Color.black)
            }
            if let s = selected, boxes.indices.contains(s) {
                Path { p in p.addRect(boxes[s].insetBy(dx: -2, dy: -2)) }
                    .stroke(Color.accentColor, lineWidth: 2.5)
            }
            if let pending {
                Path { p in p.addRect(pending) }
                    .fill(Color.black.opacity(0.35))
                Path { p in p.addRect(pending) }
                    .stroke(Color.white, style: StrokeStyle(lineWidth: 1.5, dash: [5, 4]))
            }
        }
        .allowsHitTesting(false)
    }
}

// MARK: - Compress

@MainActor
final class CompressModel: ObservableObject {
    enum Level: String, CaseIterable, Identifiable {
        case smart, light, balanced, strong
        var id: String { rawValue }
        var title: String { rawValue.capitalized }
    }

    /// Size limits of common places to send files, with room to spare.
    struct Preset: Identifiable {
        let title: String
        let megabytes: String
        var id: String { title }
    }

    static let presets = [
        Preset(title: "Email (14 MB, fits Gmail, Outlook and iCloud)", megabytes: "14"),
        Preset(title: "Discord (19 MB)", megabytes: "19"),
        Preset(title: "WhatsApp video (15 MB)", megabytes: "15"),
        Preset(title: "Website upload (5 MB)", megabytes: "5"),
        Preset(title: "Small (1 MB)", megabytes: "1"),
    ]

    /// Smart: the smallest file that still looks the same.
    @Published var level: Level = .smart { didSet { if level != oldValue { settingsChanged() } } }
    @Published var useTarget = false { didSet { if useTarget != oldValue { settingsChanged() } } }
    @Published var targetText = "1" { didSet { if targetText != oldValue { settingsChanged() } } }
    @Published var showOriginal = false
    @Published private(set) var picture: Picture?
    @Published private(set) var result: Picture?
    @Published private(set) var resultBytes: Int64?
    @Published private(set) var working = false
    @Published private(set) var message: String?
    @Published private(set) var loading = true
    let files: [URL]
    let originalBytes: Int64
    /// Videos take a while, so they're only previewed on request.
    let isVideo: Bool
    private let finish: (JSONValue?) -> Void
    private let scratch = ScratchFolder()
    private var current: PreviewRun?
    private var pending: Task<Void, Never>?
    private var lastFolder: URL?

    init(files: [URL], finish: @escaping (JSONValue?) -> Void) {
        self.files = files
        self.finish = finish
        originalBytes = Bytes.size(of: files[0])
        isVideo = PictureLoader.isVideo(files[0])
        Task { [weak self] in await self?.load() }
    }

    private func load() async {
        let p = await PictureLoader.load(files[0], scratch: scratch)
        guard !scratch.isRemoved else { return }
        picture = p
        loading = false
        if !isVideo { preview() }
    }

    var targetBytes: Int64? {
        let t = targetText.trimmingCharacters(in: .whitespaces).replacingOccurrences(of: ",", with: ".")
        guard let mb = Double(t), mb.isFinite, mb > 0,
              let bytes = Int64(exactly: (mb * 1_000_000).rounded()) else { return nil }
        return max(1, bytes)
    }

    var action: JSONValue? {
        if useTarget {
            guard let t = targetBytes else { return nil }
            return ToolAction.compress(level: level.rawValue, targetBytes: t)
        }
        return ToolAction.compress(level: level.rawValue, targetBytes: nil)
    }

    private func settingsChanged() {
        current?.cancel()
        current = nil
        working = false
        result = nil
        resultBytes = nil
        message = nil
        pending?.cancel()
        guard !isVideo, !loading else { return }
        pending = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 400_000_000)
            guard !Task.isCancelled else { return }
            self?.preview()
        }
    }

    /// Compresses the first file into a scratch folder to show the result.
    func preview() {
        current?.cancel()
        guard let action else {
            message = "Enter a size in MB, such as 2 or 0.5."
            return
        }
        if let old = lastFolder { try? FileManager.default.removeItem(at: old) }
        guard let folder = scratch.fresh() else { return }  // the window closed
        let run = PreviewRun()
        current = run
        working = true
        message = nil
        lastFolder = folder
        let file = files[0]
        Task { [weak self] in
            let outcome: Result<Outcome?, Error>
            do {
                outcome = .success(try await run.run(file: file, action: action, folder: folder))
            } catch {
                outcome = .failure(error)
            }
            guard let self else { return }
            self.scratch.tidy()
            self.previewFinished(run, outcome)
        }
    }

    private func previewFinished(_ run: PreviewRun, _ outcome: Result<Outcome?, Error>) {
        guard run === current else { return }  // settings changed meanwhile
        working = false
        switch outcome {
        case .failure(let error):
            message = error.localizedDescription
        case .success(nil):
            break
        case .success(let o?):
            if o.status == "done", let made = o.createdItems.first {
                let url = URL(fileURLWithPath: made)
                resultBytes = Int64(o.bytesOut ?? UInt64(Bytes.size(of: url)))
                if !isVideo { result = PictureLoader.image(url) }
            } else {
                message = o.error?.message ?? "Veer couldn't compress this file."
            }
        }
    }

    var sizeText: String {
        if working { return isVideo ? "Compressing a test copy…" : "Working…" }
        if let message { return message }
        let before = Bytes.text(originalBytes)
        guard let after = resultBytes else {
            return isVideo ? "Original \(before). Estimate the result to see its size." : "Original \(before)"
        }
        let first = files.count > 1 ? "First file: " : ""
        if after >= originalBytes { return "\(first)\(before) → \(Bytes.text(after)) (no smaller)" }
        let saved = Int((Double(originalBytes - after) / Double(max(originalBytes, 1)) * 100).rounded())
        return "\(first)\(before) → \(Bytes.text(after)) (\(saved)% smaller)"
    }

    var shown: Picture? { showOriginal ? picture : (result ?? picture) }

    var canApply: Bool { action != nil }

    var note: String {
        if useTarget {
            return files.count > 1 ? "Each file aims for this size or smaller." : "Veer lowers the quality just enough to fit."
        }
        if level == .smart {
            return "Smart finds the smallest JPEG or WebP that still looks the same; other files use Balanced."
        }
        return files.count > 1 ? "Compresses all \(files.count) files." : "The original stays as it is."
    }

    func cancel() { finish(nil) }
    func apply() {
        guard let action else { return }
        finish(action)
    }
    func saveRecipe() {
        RecipeSaver.save(action, suggested: useTarget ? "Compress to \(targetText) MB" : "Compress (\(level.rawValue))")
    }
    func stop() {
        current?.cancel()
        pending?.cancel()
        scratch.remove()
    }
}

struct CompressView: View {
    @ObservedObject var model: CompressModel

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                Picker("Compress by", selection: $model.useTarget) {
                    Text("Quality").tag(false)
                    Text("Target size").tag(true)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .frame(width: 190)
                if model.useTarget {
                    TextField("", text: $model.targetText)
                        .multilineTextAlignment(.trailing)
                        .frame(width: 64)
                    Text("MB").foregroundStyle(.secondary)
                    Menu("Fit for") {
                        ForEach(CompressModel.presets) { preset in
                            Button(preset.title) { model.targetText = preset.megabytes }
                        }
                    }
                    .fixedSize()
                } else {
                    Picker("Level", selection: $model.level) {
                        ForEach(CompressModel.Level.allCases) { l in Text(l.title).tag(l) }
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .frame(width: 300)
                }
                Spacer()
                if !model.isVideo {
                    Picker("Show", selection: $model.showOriginal) {
                        Text("Result").tag(false)
                        Text("Original").tag(true)
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .frame(width: 160)
                }
            }
            .padding(12)
            Divider()
            ZStack(alignment: .topTrailing) {
                PictureCanvas(picture: model.shown, loading: model.loading, overlay: { _ in EmptyView() })
                if model.working {
                    ProgressView().controlSize(.small).padding(14)
                }
            }
            Divider()
            HStack {
                Text(model.sizeText)
                    .font(.callout.monospacedDigit())
                    .foregroundStyle(.secondary)
                Spacer()
                if model.isVideo {
                    Button("Estimate Size") { model.preview() }
                        .disabled(model.working || !model.canApply)
                }
            }
            .padding(.horizontal, 16)
            .padding(.top, 10)
            ToolFooter(note: model.note, applyTitle: "Compress", canApply: model.canApply,
                       cancel: { model.cancel() }, apply: { model.apply() },
                       saveRecipe: { model.saveRecipe() })
        }
        .frame(minWidth: 560, idealWidth: 760, maxWidth: .infinity, minHeight: 440, idealHeight: 600, maxHeight: .infinity)
    }
}

// MARK: - Split PDF

@MainActor
final class SplitModel: ObservableObject {
    enum Mode: String, CaseIterable, Identifiable {
        case everyPage, chunks, ranges
        var id: String { rawValue }
    }

    @Published var mode: Mode = .everyPage
    @Published var chunkText = "2"
    @Published var ranges = ""
    let files: [URL]
    /// Page count of the only PDF, or nil for several (or unreadable) PDFs.
    let pageCount: Int?
    private let finish: (JSONValue?) -> Void

    init(files: [URL], finish: @escaping (JSONValue?) -> Void) {
        self.files = files
        self.finish = finish
        if files.count == 1, let doc = PDFDocument(url: files[0]), !doc.isLocked {
            pageCount = doc.pageCount
        } else {
            pageCount = nil
        }
    }

    var chunkSize: Int? {
        // The engine counts pages in 32 bits.
        guard let n = Int(chunkText.trimmingCharacters(in: .whitespaces)), n >= 1, n <= Int(UInt32.max) else {
            return nil
        }
        return n
    }

    func nudge(_ direction: Int) {
        chunkText = String(max(1, min((chunkSize ?? 2) + direction, Int(UInt32.max))))
    }

    var headline: String {
        if let n = pageCount { return "\(files[0].lastPathComponent) has \(n) page\(n == 1 ? "" : "s")." }
        if files.count > 1 { return "Each of the \(files.count) PDFs is split the same way." }
        return "Choose how to split \(files[0].lastPathComponent)."
    }

    /// How many PDFs this makes, or what's wrong.
    var outcome: Result<Int?, PageRanges.RangeError> {
        switch mode {
        case .everyPage:
            if let n = pageCount, n < 2 { return .failure(.init(message: "This PDF has only one page.")) }
            return .success(pageCount)
        case .chunks:
            guard let size = chunkSize else { return .failure(.init(message: "Enter how many pages go in each file.")) }
            guard let n = pageCount else { return .success(nil) }
            let parts = PageRanges.chunkCount(pages: n, size: size)
            return parts < 2 ? .failure(.init(message: "That leaves the PDF in one piece.")) : .success(parts)
        case .ranges:
            // With several PDFs, only the wording can be checked here.
            switch PageRanges.parse(ranges, count: pageCount ?? Int(UInt32.max)) {
            case .success(let groups): return .success(pageCount == nil ? nil : groups.count)
            case .failure(let e): return .failure(e)
            }
        }
    }

    var summary: (text: String, ok: Bool) {
        switch outcome {
        case .success(let n?): return ("Makes \(n) PDF\(n == 1 ? "" : "s").", true)
        case .success(nil): return ("", true)
        case .failure(let e): return (Self.sentence(e.message), false)
        }
    }

    var canApply: Bool {
        if case .success = outcome { return true }
        return false
    }

    /// The engine's messages start in lower case without a full stop.
    static func sentence(_ s: String) -> String {
        guard let first = s.first else { return s }
        let text = String(first).uppercased() + String(s.dropFirst())
        return text.hasSuffix(".") ? text : text + "."
    }

    func cancel() { finish(nil) }
    func apply() {
        guard canApply else { return }
        switch mode {
        case .everyPage: finish(ToolAction.splitEveryPage())
        case .chunks: finish(ToolAction.splitChunks(chunkSize ?? 1))
        case .ranges: finish(ToolAction.splitRanges(ranges))
        }
    }
}

struct SplitView: View {
    @ObservedObject var model: SplitModel

    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 14) {
                Text(model.headline).font(.headline)
                Picker("Split", selection: $model.mode) {
                    Text("Every page").tag(SplitModel.Mode.everyPage)
                    Text("Every few pages").tag(SplitModel.Mode.chunks)
                    Text("Page ranges").tag(SplitModel.Mode.ranges)
                }
                .pickerStyle(.radioGroup)
                .labelsHidden()
                if model.mode == .chunks {
                    HStack(spacing: 6) {
                        Text("Pages in each file")
                        TextField("", text: $model.chunkText)
                            .multilineTextAlignment(.trailing)
                            .frame(width: 56)
                        Stepper("", onIncrement: { model.nudge(1) }, onDecrement: { model.nudge(-1) })
                            .labelsHidden()
                    }
                }
                if model.mode == .ranges {
                    VStack(alignment: .leading, spacing: 4) {
                        TextField("1-3, 5, 8-", text: $model.ranges)
                            .textFieldStyle(.roundedBorder)
                        Text("Each range becomes one PDF. \"8-\" means page 8 to the end.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                Text(model.summary.text)
                    .font(.callout)
                    .foregroundColor(model.summary.ok ? .secondary : .red)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(20)
            Divider()
            ToolFooter(note: "The original PDF stays as it is.", applyTitle: "Split", canApply: model.canApply,
                       cancel: { model.cancel() }, apply: { model.apply() })
        }
        .frame(width: 460)
    }
}

// MARK: - Pages

@MainActor
final class PagesModel: ObservableObject {
    /// Original page numbers (from 1) in their new order.
    @Published var pages: [Int] = [] {
        didSet { sortedPages = pages.sorted() }
    }
    private var sortedPages: [Int] = []
    @Published var selection = Set<Int>()
    /// Clockwise turns by original page number.
    @Published private(set) var turns: [Int: Int] = [:]
    private var thumbnails: [Int: NSImage] = [:]
    let file: URL
    let count: Int
    let locked: Bool
    private let document: PDFDocument?
    private let finish: (JSONValue?) -> Void

    init(file: URL, finish: @escaping (JSONValue?) -> Void) {
        self.file = file
        self.finish = finish
        let doc = PDFDocument(url: file)
        document = doc
        locked = doc?.isLocked ?? false
        count = locked ? 0 : (doc?.pageCount ?? 0)
        pages = count > 0 ? Array(1...count) : []
        sortedPages = pages
    }

    func thumbnail(for page: Int) -> NSImage? {
        if let t = thumbnails[page] { return t }
        guard let p = document?.page(at: page - 1) else { return nil }
        // A plain cache, not published: filling it while drawing a row
        // mustn't redraw the list.
        let image = p.thumbnail(of: NSSize(width: 96, height: 128), for: .cropBox)
        thumbnails[page] = image
        return image
    }

    /// True when a page sits somewhere other than where page order puts it
    /// (deleting pages alone moves nothing).
    func isMoved(_ page: Int, at position: Int) -> Bool {
        sortedPages.indices.contains(position) && sortedPages[position] != page
    }

    func move(from source: IndexSet, to destination: Int) {
        pages.move(fromOffsets: source, toOffset: destination)
    }

    func removeSelected() {
        pages.removeAll { selection.contains($0) }
        selection = []
    }

    func reverse() { pages.reverse() }

    /// Turns the selected pages (or all, with nothing selected).
    func rotate(_ degrees: Int) {
        let targets = selection.isEmpty ? pages : pages.filter { selection.contains($0) }
        for p in targets {
            turns[p] = (((turns[p] ?? 0) + degrees) % 360 + 360) % 360
        }
    }

    func turn(of page: Int) -> Int { turns[page] ?? 0 }

    func reset() {
        pages = count > 0 ? Array(1...count) : []
        selection = []
        turns = [:]
    }

    private var rotated: Bool { turns.contains { pages.contains($0.key) && $0.value != 0 } }
    var changed: Bool { count > 0 && (pages != Array(1...count) || rotated) }
    var canApply: Bool { !pages.isEmpty && changed }

    var status: String {
        if locked { return "This PDF is password-protected." }
        if count == 0 { return "Veer couldn't read the pages of this PDF." }
        let removed = count - pages.count
        return removed > 0 ? "\(pages.count) of \(count) pages · \(removed) removed" : "\(count) page\(count == 1 ? "" : "s")"
    }

    func cancel() { finish(nil) }
    func apply() {
        guard canApply else { return }
        finish(ToolAction.pages(pages, rotate: turns))
    }
}

struct PagesView: View {
    @ObservedObject var model: PagesModel

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Text(model.status).font(.callout).foregroundStyle(.secondary)
                Spacer()
                Button { model.rotate(270) } label: { Image(systemName: "rotate.left") }
                    .help("Turn left (the selected pages, or all)")
                Button { model.rotate(90) } label: { Image(systemName: "rotate.right") }
                    .help("Turn right (the selected pages, or all)")
                Button("Reverse") { model.reverse() }
                    .disabled(model.pages.count < 2)
                Button("Reset") { model.reset() }
                    .disabled(!model.changed)
                Button("Remove") { model.removeSelected() }
                    .keyboardShortcut(.delete, modifiers: [])
                    .disabled(model.selection.isEmpty)
            }
            .padding(12)
            Divider()
            List(selection: $model.selection) {
                ForEach(Array(model.pages.enumerated()), id: \.element) { position, page in
                    HStack(spacing: 12) {
                        Group {
                            if let image = model.thumbnail(for: page) {
                                Image(nsImage: image).resizable().aspectRatio(contentMode: .fit)
                            } else {
                                Rectangle().fill(Color.primary.opacity(0.06))
                            }
                        }
                        .frame(width: 48, height: 64)
                        .rotationEffect(.degrees(Double(model.turn(of: page))))
                        .frame(width: 64, height: 64)
                        .shadow(color: .black.opacity(0.15), radius: 1, y: 0.5)
                        VStack(alignment: .leading, spacing: 2) {
                            Text("Page \(page)")
                            let notes = [model.isMoved(page, at: position) ? "moved" : nil,
                                         model.turn(of: page) != 0 ? "turned \(model.turn(of: page))°" : nil]
                                .compactMap { $0 }
                            if !notes.isEmpty {
                                Text(notes.joined(separator: " · "))
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                        }
                    }
                    .padding(.vertical, 2)
                    .tag(page)
                }
                .onMove { source, destination in model.move(from: source, to: destination) }
            }
            Divider()
            ToolFooter(note: "Drag to reorder. Select pages to turn or remove them (Delete).",
                       applyTitle: "Save PDF", canApply: model.canApply,
                       cancel: { model.cancel() }, apply: { model.apply() })
        }
        .frame(minWidth: 380, idealWidth: 440, maxWidth: .infinity, minHeight: 420, idealHeight: 620, maxHeight: .infinity)
    }
}
