import Foundation

/// Why a step or recipe can't be saved yet, in words for the editor.
public struct EditProblem: Error, Equatable, Sendable {
    public let message: String

    public init(_ message: String) {
        self.message = message
    }
}

/// A recipe step as the Settings editor shows it: typed fields for the
/// options people change, with everything else kept as it was.
public struct StepFields: Equatable, Sendable {
    public enum SizeMode: String, CaseIterable, Identifiable, Sendable {
        case max, percent, width, height
        public var id: String { rawValue }

        public var title: String {
            switch self {
            case .max: return "Longest side"
            case .percent: return "Percent"
            case .width: return "Width"
            case .height: return "Height"
            }
        }

        public var unit: String { self == .percent ? "%" : "px" }
    }

    public static let levels = ["smart", "light", "balanced", "strong"]
    public static let subtitleFormats = ["srt", "vtt", "txt"]
    public static let angles = [90, 180, 270]

    public let op: String
    public var format = "jpeg"
    /// Empty for the format's default.
    public var quality = ""
    public var sizeMode = SizeMode.max
    public var size = "2048"
    public var degrees = 90
    public var level = "balanced"
    public var text = ""
    public var opacity = 0.15
    public var numberStyle = ToolAction.NumberStyle.number
    public var numberPosition = ToolAction.NumberPosition.bottomCenter
    /// Trim times as typed, such as "1:30".
    public var start = ""
    public var end = ""
    public var crop = false
    /// Transcribe: "srt", "vtt" or "txt", and a language tag or "".
    public var subtitleFormat = "srt"
    public var language = ""
    /// Trace: keep the colours, and how closely to follow the picture.
    public var traceColor = true
    public var traceDetail = StepText.TraceDetail.medium
    /// The step as loaded, so options the editor doesn't show survive.
    public let original: [String: JSONValue]

    public init(_ step: JSONValue) {
        var o: [String: JSONValue] = [:]
        if case .object(let fields) = step { o = fields }
        original = o
        op = o["op"]?.string ?? ""
        switch op {
        case "convert":
            format = o["to"]?.string ?? "jpeg"
            quality = o["quality"]?.int.map(String.init) ?? ""
        case "resize":
            for mode in SizeMode.allCases {
                if let n = o[mode.rawValue]?.int {
                    sizeMode = mode
                    size = String(n)
                    break
                }
            }
        case "rotate":
            degrees = o["degrees"]?.int ?? 90
        case "compress":
            level = o["level"]?.string ?? "balanced"
        case "watermark":
            text = o["text"]?.string ?? ""
            opacity = o["opacity"]?.double ?? 0.15
        case "page_numbers":
            numberStyle = ToolAction.NumberStyle(rawValue: o["style"]?.string ?? "") ?? .number
            numberPosition = ToolAction.NumberPosition(rawValue: o["position"]?.string ?? "") ?? .bottomCenter
        case "trim":
            start = TimeText.format(o["start"]?.double ?? 0)
            end = o["end"]?.double.map(TimeText.format) ?? ""
        case "remove_background":
            crop = o["crop"]?.bool ?? false
        case "trace":
            traceColor = o["color"]?.bool ?? true
            traceDetail = StepText.TraceDetail(rawValue: o["detail"]?.string ?? "") ?? .medium
        case "transcribe":
            subtitleFormat = o["to"]?.string ?? "srt"
            language = o["language"]?.string ?? ""
        default:
            break
        }
    }

    /// Steps whose options the editor shows; the rest (Crop, Redact,
    /// Pages…) are kept exactly as recorded.
    public var hasOptions: Bool {
        ["convert", "resize", "rotate", "compress", "watermark", "page_numbers", "trim", "remove_background",
         "transcribe", "trace"]
            .contains(op)
    }

    /// Convert targets for the picker, including one the list lacks.
    public var formatChoices: [String] {
        StepText.convertTargets.contains(format) ? StepText.convertTargets : StepText.convertTargets + [format]
    }

    /// The step as the engine reads it, or what to fix first.
    public func build() -> Result<JSONValue, EditProblem> {
        var o = original
        o["op"] = .string(op)
        func number(_ text: String) -> Int? {
            Int(text.trimmingCharacters(in: .whitespaces))
        }
        switch op {
        case "convert":
            o["to"] = .string(format)
            if quality.trimmingCharacters(in: .whitespaces).isEmpty {
                o.removeValue(forKey: "quality")
            } else if let q = number(quality), (1...100).contains(q) {
                o["quality"] = .int(Int64(q))
            } else {
                return .failure(EditProblem("Quality is a number from 1 to 100, or empty for the usual."))
            }
        case "resize":
            for mode in SizeMode.allCases { o.removeValue(forKey: mode.rawValue) }
            guard let n = number(size), n > 0 else {
                return .failure(EditProblem("Type a size for Resize, such as 2048."))
            }
            if sizeMode == .percent && n > 100 {
                return .failure(EditProblem("Resize never enlarges, so use 100% or less."))
            }
            o[sizeMode.rawValue] = .int(Int64(n))
        case "rotate":
            guard Self.angles.contains(degrees) else {
                return .failure(EditProblem("Rotate by 90°, 180° or 270°."))
            }
            o["degrees"] = .int(Int64(degrees))
        case "compress":
            o["level"] = .string(Self.levels.contains(level) ? level : "balanced")
        case "watermark":
            let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !t.isEmpty else { return .failure(EditProblem("Type the watermark's text.")) }
            o["text"] = .string(t)
            o["opacity"] = .double((min(max(opacity, 0.05), 1) * 100).rounded() / 100)
        case "page_numbers":
            o["style"] = .string(numberStyle.rawValue)
            o["position"] = .string(numberPosition.rawValue)
        case "trim":
            // Times are shown to a tenth of a second; one left as shown
            // keeps its exact value.
            func time(_ text: String, _ key: String) -> Double? {
                if let exact = original[key]?.double, text == TimeText.format(exact) { return exact }
                return TimeText.parse(text)
            }
            let s: Double
            if start.trimmingCharacters(in: .whitespaces).isEmpty {
                s = 0
            } else if let parsed = time(start, "start") {
                s = parsed
            } else {
                return .failure(EditProblem("Type Trim's start as seconds or minutes, such as 1:30."))
            }
            o["start"] = .double(s)
            if end.trimmingCharacters(in: .whitespaces).isEmpty {
                o.removeValue(forKey: "end")
            } else if let e = time(end, "end"), e > s {
                o["end"] = .double(e)
            } else {
                return .failure(EditProblem("Trim's end must come after its start (or leave it empty)."))
            }
        case "remove_background":
            if crop { o["crop"] = .bool(true) } else { o.removeValue(forKey: "crop") }
        case "trace":
            if traceColor { o.removeValue(forKey: "color") } else { o["color"] = .bool(false) }
            if traceDetail == .medium {
                o.removeValue(forKey: "detail")
            } else {
                o["detail"] = .string(traceDetail.rawValue)
            }
        case "transcribe":
            o["to"] = .string(Self.subtitleFormats.contains(subtitleFormat) ? subtitleFormat : "srt")
            let tag = language.trimmingCharacters(in: .whitespaces)
            if tag.isEmpty {
                o.removeValue(forKey: "language")
            } else if ToolAction.isLanguageTag(tag) {
                o["language"] = .string(tag)
            } else {
                return .failure(EditProblem("The language is a code such as en-US, or empty for the Mac's."))
            }
        default:
            break
        }
        return .success(.object(o))
    }
}

/// Checks a whole recipe before it's sent to the engine (which checks
/// again).
public enum RecipeCheck {
    public static let maxSteps = 16

    public static func build(name: String, description: String,
                             steps: [StepFields]) -> Result<JSONValue, EditProblem> {
        let n = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !n.isEmpty else { return .failure(EditProblem("Give the loadout a name.")) }
        guard !steps.isEmpty else { return .failure(EditProblem("Add at least one step.")) }
        guard steps.count <= maxSteps else {
            return .failure(EditProblem("A loadout can have up to \(maxSteps) steps."))
        }
        var built: [JSONValue] = []
        for (i, s) in steps.enumerated() {
            switch s.build() {
            case .success(let v): built.append(v)
            case .failure(let p): return .failure(EditProblem("Step \(i + 1): \(p.message)"))
            }
        }
        var o: [String: JSONValue] = ["name": .string(n), "steps": .array(built)]
        let d = description.trimmingCharacters(in: .whitespacesAndNewlines)
        if !d.isEmpty { o["description"] = .string(d) }
        return .success(.object(o))
    }
}

/// One recipe from the engine's `recipes` list.
public struct RecipeEntry: Identifiable, Equatable, Sendable {
    public let id: String
    public let name: String
    public let description: String
    public let steps: [JSONValue]
    /// The recipe's file; nil for built-in recipes.
    public let path: URL?
    public var builtin: Bool { path == nil }

    /// Identifies a recipe file however its path was spelled.
    public static func key(_ url: URL) -> String {
        url.resolvingSymlinksInPath().standardizedFileURL.path
    }
}

/// The engine's answer to `recipes`: built-ins first, then the user's
/// files, plus files that couldn't be read.
public struct RecipeListing: Equatable, Sendable {
    public let recipes: [RecipeEntry]
    /// "name.toml: what's wrong", one per broken file.
    public let problems: [String]

    public init(_ result: JSONValue) {
        var list: [RecipeEntry] = []
        if case .array(let found)? = result["recipes"] {
            for r in found {
                guard let name = r["name"]?.string else { continue }
                var steps: [JSONValue] = []
                if case .array(let s)? = r["steps"] { steps = s }
                let path = r["path"]?.string.map { URL(fileURLWithPath: $0) }
                list.append(RecipeEntry(id: path.map(RecipeEntry.key) ?? "builtin:\(name)", name: name,
                                        description: r["description"]?.string ?? "", steps: steps, path: path))
            }
        }
        recipes = list
        var errors: [String] = []
        if case .array(let found)? = result["errors"] {
            for e in found {
                let file = e["path"]?.string.map { URL(fileURLWithPath: $0).lastPathComponent } ?? "A loadout file"
                errors.append("\(file): \(e["message"]?.string ?? "couldn't be read")")
            }
        }
        problems = errors
    }
}
