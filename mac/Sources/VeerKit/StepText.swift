import Foundation

/// Plain-English descriptions of recipe steps, and the steps a recipe can
/// start from.
public enum StepText {
    static let formatNames: [String: String] = [
        "jpeg": "JPEG", "png": "PNG", "webp": "WebP", "heic": "HEIC", "avif": "AVIF", "gif": "GIF",
        "tiff": "TIFF", "bmp": "BMP", "pdf": "PDF", "txt": "Text", "md": "Markdown", "html": "HTML",
        "mp4": "MP4", "mov": "MOV", "webm": "WebM", "mkv": "MKV", "mp3": "MP3", "m4a": "M4A",
        "wav": "WAV", "flac": "FLAC", "ogg": "OGG", "opus": "Opus", "srt": "SRT", "vtt": "WebVTT",
    ]

    public static func formatName(_ id: String) -> String {
        formatNames[id] ?? id.uppercased()
    }

    /// Formats offered when adding a Convert step.
    public static let convertTargets = ["jpeg", "png", "webp", "heic", "pdf", "gif", "tiff", "txt",
                                        "mp4", "mov", "webm", "mp3", "m4a", "wav"]

    public static func describe(_ step: JSONValue) -> String {
        let op = step["op"]?.string ?? "?"
        switch op {
        case "convert":
            let to = formatName(step["to"]?.string ?? "?")
            if let q = step["quality"]?.int { return "Convert to \(to) (quality \(q))" }
            return "Convert to \(to)"
        case "resize":
            if let p = step["percent"]?.int { return "Resize to \(p)%" }
            if let m = step["max"]?.int { return "Longest side \(m) px" }
            if let w = step["width"]?.int { return "Width \(w) px" }
            if let h = step["height"]?.int { return "Height \(h) px" }
            return "Resize"
        case "rotate":
            return "Rotate \(step["degrees"]?.int ?? 90)°"
        case "crop":
            return "Crop"
        case "redact":
            let n: Int
            if case .array(let a)? = step["rects"] { n = a.count } else { n = 0 }
            return "Redact \(n) area\(n == 1 ? "" : "s")"
        case "strip_metadata":
            return "Remove metadata"
        case "compress":
            if let t = step["target_bytes"]?.double {
                return "Compress to " + ByteCountFormatter.string(fromByteCount: Int64(t), countStyle: .file)
            }
            if step["level"]?.string == "smart" { return "Compress (looks the same)" }
            return "Compress (\(step["level"]?.string ?? "balanced"))"
        case "pages":
            if case .array(let a)? = step["pages"] {
                return "Keep pages " + a.compactMap { $0.int.map(String.init) }.joined(separator: ", ")
            }
            return "Keep pages"
        case "page_numbers":
            let style = ToolAction.NumberStyle(rawValue: step["style"]?.string ?? "number") ?? .number
            return "Page numbers (\(style.example))"
        case "watermark":
            return "Watermark \u{201C}\(step["text"]?.string ?? "")\u{201D}"
        case "trim":
            let start = TimeText.format(step["start"]?.double ?? 0)
            if let end = step["end"]?.double { return "Trim \(start) to \(TimeText.format(end))" }
            return "Trim from \(start)"
        case "normalize_audio":
            return "Even out volume"
        case "remove_background":
            return step["crop"]?.bool == true ? "Remove background and trim" : "Remove background"
        case "make_searchable":
            return "Make searchable PDF"
        case "transcribe":
            let kind: String
            switch step["to"]?.string ?? "srt" {
            case "vtt": kind = "Subtitles (WebVTT)"
            case "txt": kind = "Transcript"
            default: kind = "Subtitles (SRT)"
            }
            if let language = step["language"]?.string, !language.isEmpty { return "\(kind), \(language)" }
            return kind

        default:
            return op.replacingOccurrences(of: "_", with: " ").capitalized
        }
    }

    /// Steps a new recipe step can start as, with a title for the menu.
    public static let templates: [(title: String, step: JSONValue)] = [
        ("Convert", .object(["op": .string("convert"), "to": .string("jpeg")])),
        ("Resize", .object(["op": .string("resize"), "max": .int(2048)])),
        ("Rotate", .object(["op": .string("rotate"), "degrees": .int(90)])),
        ("Compress", .object(["op": .string("compress"), "level": .string("balanced")])),
        ("Remove metadata", .object(["op": .string("strip_metadata")])),
        ("Remove background", .object(["op": .string("remove_background")])),
        ("Make searchable PDF", .object(["op": .string("make_searchable")])),
        ("Page numbers", .object(["op": .string("page_numbers"), "style": .string("number")])),
        ("Watermark", .object(["op": .string("watermark"), "text": .string("DRAFT")])),
        ("Even out volume", .object(["op": .string("normalize_audio")])),
        ("Make subtitles", .object(["op": .string("transcribe"), "to": .string("srt")])),
    ]
}
