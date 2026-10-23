import CoreGraphics
import Foundation

/// A rectangle as fractions (0 to 1) of an upright picture, origin at the
/// top left. This is the engine's `Rect`, so it means the same thing
/// whatever the image's pixel size or orientation tag.
public struct NormRect: Equatable, Sendable {
    public var x: Double
    public var y: Double
    public var w: Double
    public var h: Double

    public init(x: Double, y: Double, w: Double, h: Double) {
        self.x = x
        self.y = y
        self.w = w
        self.h = h
    }

    public static let full = NormRect(x: 0, y: 0, w: 1, h: 1)

    /// The rectangle spanned by two points (in fractions), clamped to the
    /// picture.
    public init(from a: CGPoint, to b: CGPoint) {
        let x0 = min(max(min(a.x, b.x), 0), 1)
        let y0 = min(max(min(a.y, b.y), 0), 1)
        let x1 = min(max(max(a.x, b.x), 0), 1)
        let y1 = min(max(max(a.y, b.y), 0), 1)
        self.init(x: Double(x0), y: Double(y0), w: Double(x1 - x0), h: Double(y1 - y0))
    }

    /// Big enough to mean something (not an accidental click).
    public var isUsable: Bool { w >= 0.01 && h >= 0.01 }

    public var isFull: Bool { x <= 0.0005 && y <= 0.0005 && w >= 0.999 && h >= 0.999 }

    public func contains(_ p: CGPoint) -> Bool {
        Double(p.x) >= x && Double(p.x) <= x + w && Double(p.y) >= y && Double(p.y) <= y + h
    }

    /// Moved by (dx, dy), kept inside the picture.
    public func moved(dx: Double, dy: Double) -> NormRect {
        NormRect(x: min(max(x + dx, 0), 1 - w), y: min(max(y + dy, 0), 1 - h), w: w, h: h)
    }

    /// The engine's JSON form.
    public var json: JSONValue {
        // Six decimals is far finer than any pixel; rounding x and w
        // separately must never push the box past the edge.
        func r(_ v: Double) -> Double { (min(max(v, 0), 1) * 1_000_000).rounded() / 1_000_000 }
        func room(_ start: Double) -> Double { ((1 - start) * 1_000_000).rounded(.down) / 1_000_000 }
        let rx = r(x), ry = r(y)
        let rw = min(r(w), room(rx))
        let rh = min(r(h), room(ry))
        return .object(["x": .double(rx), "y": .double(ry), "w": .double(rw), "h": .double(rh)])
    }

    /// Pixel size of this rectangle in a picture of `size` pixels.
    public func pixels(of size: CGSize) -> (width: Int, height: Int) {
        (max(1, Int((w * Double(size.width)).rounded())), max(1, Int((h * Double(size.height)).rounded())))
    }
}

/// Where an aspect-fit picture sits inside a view, and conversions
/// between view points and picture fractions.
public enum ImageFit {
    public static func frame(content: CGSize, in container: CGSize, padding: CGFloat = 12) -> CGRect {
        let avail = CGSize(width: max(container.width - 2 * padding, 1), height: max(container.height - 2 * padding, 1))
        guard content.width > 0, content.height > 0 else {
            return CGRect(x: padding, y: padding, width: avail.width, height: avail.height)
        }
        let scale = min(avail.width / content.width, avail.height / content.height)
        let size = CGSize(width: content.width * scale, height: content.height * scale)
        return CGRect(x: (container.width - size.width) / 2, y: (container.height - size.height) / 2,
                      width: size.width, height: size.height)
    }

    /// A view point as picture fractions, clamped to the picture.
    public static func fraction(of p: CGPoint, in frame: CGRect) -> CGPoint {
        guard frame.width > 0, frame.height > 0 else { return .zero }
        return CGPoint(x: min(max((p.x - frame.minX) / frame.width, 0), 1),
                       y: min(max((p.y - frame.minY) / frame.height, 0), 1))
    }

    /// A fraction rectangle in view coordinates.
    public static func viewRect(_ r: NormRect, in frame: CGRect) -> CGRect {
        CGRect(x: frame.minX + CGFloat(r.x) * frame.width, y: frame.minY + CGFloat(r.y) * frame.height,
               width: CGFloat(r.w) * frame.width, height: CGFloat(r.h) * frame.height)
    }
}

/// Crop-box editing: drag inside to move, near a corner to resize, outside
/// to draw a new box. Optional fixed aspect ratio (in pixels).
public struct CropEditor: Equatable, Sendable {
    public var rect: NormRect = .full
    /// Width ÷ height in pixels, or nil for free cropping.
    public var aspect: Double?
    /// The picture's pixel size, needed to keep aspect ratios true.
    public var pictureSize: CGSize

    private enum Drag: Equatable {
        case move(NormRect, CGPoint)
        /// `offset` keeps the grabbed corner from jumping to the pointer.
        case resize(anchor: CGPoint, offset: CGPoint)
        case create(anchor: CGPoint)
    }
    private var drag: Drag?

    public init(pictureSize: CGSize) {
        self.pictureSize = pictureSize
    }

    /// Corner grab distance, in fractions.
    static let grab = 0.04

    public mutating func begin(at p: CGPoint) {
        let corners: [(CGPoint, CGPoint)] = [
            (CGPoint(x: rect.x, y: rect.y), CGPoint(x: rect.x + rect.w, y: rect.y + rect.h)),
            (CGPoint(x: rect.x + rect.w, y: rect.y), CGPoint(x: rect.x, y: rect.y + rect.h)),
            (CGPoint(x: rect.x, y: rect.y + rect.h), CGPoint(x: rect.x + rect.w, y: rect.y)),
            (CGPoint(x: rect.x + rect.w, y: rect.y + rect.h), CGPoint(x: rect.x, y: rect.y)),
        ]
        if let hit = corners.first(where: { abs(Double($0.0.x - p.x)) < Self.grab && abs(Double($0.0.y - p.y)) < Self.grab }) {
            drag = .resize(anchor: hit.1, offset: CGPoint(x: hit.0.x - p.x, y: hit.0.y - p.y))
        } else if rect.contains(p) && !rect.isFull {
            drag = .move(rect, p)
        } else {
            drag = .create(anchor: p)
        }
    }

    public mutating func update(to p: CGPoint) {
        switch drag {
        case .move(let start, let origin)?:
            rect = start.moved(dx: Double(p.x - origin.x), dy: Double(p.y - origin.y))
        case .resize(let anchor, let offset)?:
            let r = shaped(from: anchor, to: CGPoint(x: p.x + offset.x, y: p.y + offset.y))
            if r.isUsable { rect = r }
        case .create(let anchor)?:
            let r = shaped(from: anchor, to: p)
            if r.isUsable { rect = r }
        case nil:
            break
        }
    }

    public mutating func end() {
        drag = nil
    }

    /// Applies a new aspect ratio, keeping the box centred.
    public mutating func setAspect(_ a: Double?) {
        aspect = a
        guard let a, pictureSize.width > 0, pictureSize.height > 0 else { return }
        let ratio = a * Double(pictureSize.height) / Double(pictureSize.width)  // w/h in fractions
        var w = rect.w, h = rect.w / ratio
        if h > 1 { h = 1; w = ratio }
        if w > 1 { w = 1; h = 1 / ratio }
        let cx = rect.x + rect.w / 2, cy = rect.y + rect.h / 2
        rect = NormRect(x: min(max(cx - w / 2, 0), 1 - w), y: min(max(cy - h / 2, 0), 1 - h), w: w, h: h)
    }

    public mutating func reset() {
        rect = .full
        if let a = aspect { setAspect(a) }
    }

    /// Box from `anchor` towards `p`, honouring the aspect ratio and
    /// staying inside the picture.
    private func shaped(from anchor: CGPoint, to p: CGPoint) -> NormRect {
        guard let a = aspect, pictureSize.width > 0, pictureSize.height > 0 else {
            return NormRect(from: anchor, to: p)
        }
        let ratio = a * Double(pictureSize.height) / Double(pictureSize.width)
        let dx = Double(p.x - anchor.x), dy = Double(p.y - anchor.y)
        let sx: Double = dx < 0 ? -1 : 1, sy: Double = dy < 0 ? -1 : 1
        let maxW = sx > 0 ? 1 - Double(anchor.x) : Double(anchor.x)
        let maxH = sy > 0 ? 1 - Double(anchor.y) : Double(anchor.y)
        var w = max(abs(dx), abs(dy) * ratio)
        w = min(w, maxW, maxH * ratio)
        let h = w / ratio
        let end = CGPoint(x: anchor.x + CGFloat(sx * w), y: anchor.y + CGFloat(sy * h))
        return NormRect(from: anchor, to: end)
    }
}

/// Redaction-box editing: drag on empty space to add a box, click a box to
/// select it.
public struct RedactEditor: Equatable, Sendable {
    public var rects: [NormRect] = []
    public var selected: Int?
    private var anchor: CGPoint?
    private var drawing: NormRect?

    public init() {}

    /// The box being drawn, for live display.
    public var pending: NormRect? { drawing }

    public mutating func begin(at p: CGPoint) {
        if let i = rects.lastIndex(where: { $0.contains(p) }) {
            selected = i
            anchor = nil
        } else {
            selected = nil
            anchor = p
        }
        drawing = nil
    }

    public mutating func update(to p: CGPoint) {
        guard let a = anchor else { return }
        drawing = NormRect(from: a, to: p)
    }

    public mutating func end() {
        if let d = drawing, d.isUsable {
            rects.append(d)
            selected = rects.count - 1
        }
        anchor = nil
        drawing = nil
    }

    public mutating func removeSelected() {
        guard let s = selected, rects.indices.contains(s) else { return }
        rects.remove(at: s)
        selected = nil
    }
}

/// Builds the engine actions the tool windows run.
public enum ToolAction {
    public enum ResizeMode: String, CaseIterable, Identifiable, Sendable {
        case percent, max, width, height
        public var id: String { rawValue }
        public var title: String {
            switch self {
            case .percent: return "Percentage"
            case .max: return "Longest side"
            case .width: return "Width"
            case .height: return "Height"
            }
        }
    }

    static func steps(_ steps: [JSONValue]) -> JSONValue {
        .object(["kind": .string("steps"), "steps": .array(steps)])
    }

    public static func resize(_ mode: ResizeMode, _ value: Int) -> JSONValue {
        steps([.object(["op": .string("resize"), mode.rawValue: .int(Int64(value))])])
    }

    /// Resulting pixel size for a picture of `size`, or nil if unchanged.
    /// A `vector` drawing (SVG) is drawn afresh, so it can grow: the
    /// longest side becomes `max`, and percentages may pass 100.
    public static func resizedSize(_ size: CGSize, _ mode: ResizeMode, _ value: Int,
                                   vector: Bool = false) -> CGSize? {
        let w = Double(size.width), h = Double(size.height)
        guard w > 0, h > 0, value > 0 else { return nil }
        let v = Double(value)
        let out: (Double, Double)
        switch mode {
        case .percent where vector: out = ((w * v / 100).rounded(), (h * v / 100).rounded())
        case .percent: out = value >= 100 ? (w, h) : ((w * v / 100).rounded(.down), (h * v / 100).rounded(.down))
        case .max:
            let s = vector ? v / max(w, h) : min(1, v / max(w, h))
            out = ((w * s).rounded(), (h * s).rounded())
        case .width: out = (v, (h * v / w).rounded())
        case .height: out = ((w * v / h).rounded(), v)
        }
        return CGSize(width: max(1, out.0), height: max(1, out.1))
    }

    public static func crop(_ rect: NormRect) -> JSONValue {
        steps([.object(["op": .string("crop"), "rect": rect.json])])
    }

    public static func redact(_ rects: [NormRect], style: String) -> JSONValue {
        steps([.object(["op": .string("redact"), "rects": .array(rects.map(\.json)), "style": .string(style)])])
    }

    /// `level` is light, balanced or strong; `targetBytes` overrides it.
    public static func compress(level: String, targetBytes: Int64?) -> JSONValue {
        var step: [String: JSONValue] = ["op": .string("compress"), "level": .string(level)]
        if let t = targetBytes { step["target_bytes"] = .int(t) }
        return steps([.object(step)])
    }

    public static func splitEveryPage() -> JSONValue {
        .object(["kind": .string("split_pdf"), "mode": .object(["by": .string("every_page")])])
    }

    public static func splitChunks(_ size: Int) -> JSONValue {
        .object(["kind": .string("split_pdf"), "mode": .object(["by": .string("chunks"), "size": .int(Int64(size))])])
    }

    public static func splitRanges(_ ranges: String) -> JSONValue {
        .object(["kind": .string("split_pdf"), "mode": .object(["by": .string("ranges"), "ranges": .string(ranges)])])
    }

    /// Keep `pages` (numbered from 1) in this order.
    public static func pages(_ pages: [Int]) -> JSONValue {
        steps([.object(["op": .string("pages"), "pages": .array(pages.map { .int(Int64($0)) })])])
    }
}

/// Page ranges such as "1-3, 5, 8-", read exactly as the engine reads
/// them, so the Split window can check them as you type.
public enum PageRanges {
    /// One range of page numbers per part, or a message saying what's
    /// wrong. An open start or end runs to the first or last page.
    public static func parse(_ spec: String, count: Int) -> Result<[ClosedRange<Int>], RangeError> {
        var groups: [ClosedRange<Int>] = []
        let parts = spec.split(whereSeparator: { $0 == "," || $0 == ";" })
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
        for part in parts {
            let a: String, b: String
            if let dash = part.firstIndex(of: "-") {
                a = part[..<dash].trimmingCharacters(in: .whitespacesAndNewlines)
                b = part[part.index(after: dash)...].trimmingCharacters(in: .whitespacesAndNewlines)
            } else {
                a = part
                b = part
            }
            let bad = RangeError(message: "\"\(part)\" isn't a page range (try 1-3, 5, 8-)")
            guard let start = a.isEmpty ? 1 : UInt32(a).map(Int.init),
                  let end = b.isEmpty ? count : UInt32(b).map(Int.init) else {
                return .failure(bad)
            }
            if start == 0 || start > end { return .failure(bad) }
            if end > count { return .failure(RangeError(message: "page \(end) doesn't exist (the PDF has \(count))")) }
            groups.append(start...end)
        }
        if groups.isEmpty { return .failure(RangeError(message: "enter at least one page range")) }
        return .success(groups)
    }

    public struct RangeError: Error, Equatable, Sendable {
        public let message: String
        public init(message: String) {
            self.message = message
        }
    }

    /// How many PDFs splitting into chunks of `size` pages makes.
    public static func chunkCount(pages: Int, size: Int) -> Int {
        guard pages > 0, size > 0 else { return 0 }
        return (pages - 1) / size + 1
    }
}

// MARK: - Actions for the newer tool windows

extension ToolAction {
    public enum NumberPosition: String, CaseIterable, Identifiable, Sendable {
        case topLeft = "top_left", topCenter = "top_center", topRight = "top_right"
        case bottomLeft = "bottom_left", bottomCenter = "bottom_center", bottomRight = "bottom_right"
        public var id: String { rawValue }
    }

    public enum NumberStyle: String, CaseIterable, Identifiable, Sendable {
        case number, page
        case pageOfTotal = "page_of_total"
        case fraction
        public var id: String { rawValue }

        /// How page 3 of 12 reads.
        public var example: String {
            switch self {
            case .number: return "3"
            case .page: return "Page 3"
            case .pageOfTotal: return "Page 3 of 12"
            case .fraction: return "3 / 12"
            }
        }
    }

    public static func pageNumbers(position: NumberPosition, style: NumberStyle, start: Int,
                                   size: Double, skipFirst: Bool) -> JSONValue {
        var step: [String: JSONValue] = [
            "op": .string("page_numbers"),
            "position": .string(position.rawValue),
            "style": .string(style.rawValue),
            "start": .int(Int64(start)),
            "size": .double(size),
        ]
        if skipFirst { step["skip_first"] = .bool(true) }
        return steps([.object(step)])
    }

    public static func watermark(text: String, opacity: Double, diagonal: Bool) -> JSONValue {
        steps([.object([
            "op": .string("watermark"),
            "text": .string(text),
            "opacity": .double((opacity * 100).rounded() / 100),
            "diagonal": .bool(diagonal),
        ])])
    }

    public static func protectPDF(password: String) -> JSONValue {
        .object(["kind": .string("protect_pdf"), "password": .string(password)])
    }

    public static func unlockPDF(password: String) -> JSONValue {
        .object(["kind": .string("unlock_pdf"), "password": .string(password)])
    }

    static func trimStep(start: Double, end: Double?) -> JSONValue {
        func r(_ v: Double) -> JSONValue { .double((v * 1000).rounded() / 1000) }
        var step: [String: JSONValue] = ["op": .string("trim"), "start": r(start)]
        if let end { step["end"] = r(end) }
        return .object(step)
    }

    /// Keep `start` to `end` seconds (nil: to the end).
    public static func trim(start: Double, end: Double?) -> JSONValue {
        steps([trimStep(start: start, end: end)])
    }

    /// A GIF of `start` to `end` seconds, or the whole clip.
    public static func gif(start: Double, end: Double?) -> JSONValue {
        let convert: JSONValue = .object(["op": .string("convert"), "to": .string("gif")])
        if start <= 0 && end == nil { return steps([convert]) }
        return steps([trimStep(start: start, end: end), convert])
    }

    /// Subtitles ("srt", "vtt") or a transcript ("txt") of the speech, in
    /// `language` (such as "en-US"; nil for the Mac's language).
    public static func transcribe(to format: String, language: String?) -> JSONValue {
        var step: [String: JSONValue] = ["op": .string("transcribe"), "to": .string(format)]
        if let language, !language.isEmpty { step["language"] = .string(language) }
        return steps([.object(step)])
    }

    /// A language tag the engine accepts: letters, digits, "-" and "_".
    public static func isLanguageTag(_ tag: String) -> Bool {
        let allowed = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_")
        return (2...24).contains(tag.count) && tag.unicodeScalars.allSatisfy { allowed.contains($0) }
    }

    /// Keep `pages` in this order, turning those in `rotate` (page → degrees).
    public static func pages(_ pages: [Int], rotate: [Int: Int]) -> JSONValue {
        var step: [String: JSONValue] = ["op": .string("pages"), "pages": .array(pages.map { .int(Int64($0)) })]
        let turns = rotate.filter { pages.contains($0.key) && $0.value % 360 != 0 }.sorted { $0.key < $1.key }
        if !turns.isEmpty {
            step["rotate"] = .array(turns.map {
                .object(["page": .int(Int64($0.key)), "degrees": .int(Int64(($0.value % 360 + 360) % 360))])
            })
        }
        return steps([.object(step)])
    }
}

/// Times as people type them: "90", "1:30", "1:02:03.5".
public enum TimeText {
    public static func parse(_ text: String) -> Double? {
        let parts = text.trimmingCharacters(in: .whitespaces).split(separator: ":", omittingEmptySubsequences: false)
        guard !parts.isEmpty, parts.count <= 3 else { return nil }
        var total = 0.0
        for p in parts {
            guard let v = Double(p.trimmingCharacters(in: .whitespaces)), v.isFinite, v >= 0 else { return nil }
            total = total * 60 + v
        }
        return total
    }

    /// "1:05.3", or "1:02:03.0" past an hour.
    public static func format(_ seconds: Double) -> String {
        let s = max(seconds, 0)
        let tenths = Int((s * 10).rounded())
        let h = tenths / 36_000, m = (tenths / 600) % 60, sec = (tenths / 10) % 60, t = tenths % 10
        let tail = String(format: "%02d.%d", sec, t)
        return h > 0 ? "\(h):" + String(format: "%02d:", m) + tail : "\(m):" + tail
    }
}
