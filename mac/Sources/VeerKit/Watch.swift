import Foundation

/// What a watched folder does with new files: a built-in action or a
/// recipe, looked up by name each time so edits to the recipe apply.
public enum WatchTask: Codable, Hashable, Sendable {
    case preset(String)
    case recipe(String)

    public var title: String {
        switch self {
        case .preset(let id): return WatchPresets.find(id)?.title ?? id
        case .recipe(let name): return "Recipe: \(name)"
        }
    }
}

/// A folder Veer watches for new files.
public struct WatchRule: Codable, Identifiable, Equatable, Sendable {
    public var id: UUID
    public var path: String
    public var task: WatchTask
    public var enabled: Bool

    public init(id: UUID = UUID(), path: String, task: WatchTask, enabled: Bool = true) {
        self.id = id
        self.path = path
        self.task = task
        self.enabled = enabled
    }

    public var url: URL { URL(fileURLWithPath: path, isDirectory: true) }

    /// Results go into a subfolder, so they never trigger the rule again
    /// and the originals stay untouched.
    public var outputURL: URL { url.appendingPathComponent(WatchPresets.outputFolder, isDirectory: true) }

    public static func decodeList(_ data: Data?) -> [WatchRule] {
        guard let data, let rules = try? JSONDecoder().decode([WatchRule].self, from: data) else { return [] }
        return rules
    }

    public static func encodeList(_ rules: [WatchRule]) -> Data? {
        try? JSONEncoder().encode(rules)
    }
}

public struct WatchPreset: Sendable {
    public let id: String
    public let title: String
    public let action: JSONValue
}

public enum WatchPresets {
    public static let outputFolder = "Converted"

    private static func steps(_ steps: [JSONValue]) -> JSONValue {
        .object(["kind": .string("steps"), "steps": .array(steps)])
    }

    private static func convert(_ to: String) -> JSONValue {
        steps([.object(["op": .string("convert"), "to": .string(to)])])
    }

    public static let all: [WatchPreset] = [
        WatchPreset(id: "to_jpeg", title: "Convert to JPEG", action: convert("jpeg")),
        WatchPreset(id: "to_png", title: "Convert to PNG", action: convert("png")),
        WatchPreset(id: "to_webp", title: "Convert to WebP", action: convert("webp")),
        WatchPreset(id: "to_pdf", title: "Convert to PDF", action: convert("pdf")),
        WatchPreset(id: "to_mp4", title: "Convert to MP4", action: convert("mp4")),
        WatchPreset(id: "to_mp3", title: "Convert to MP3", action: convert("mp3")),
        WatchPreset(id: "compress", title: "Compress",
                    action: steps([.object(["op": .string("compress"), "level": .string("balanced")])])),
        WatchPreset(id: "resize_1920", title: "Resize to fit 1920 px",
                    action: steps([.object(["op": .string("resize"), "max": .int(1920)])])),
        WatchPreset(id: "strip_metadata", title: "Remove metadata",
                    action: steps([.object(["op": .string("strip_metadata")])])),
    ]

    public static func find(_ id: String) -> WatchPreset? {
        all.first { $0.id == id }
    }

    /// A recipe's action from the engine's `recipes` reply, by name.
    public static func recipeAction(named name: String, in reply: JSONValue) -> JSONValue? {
        guard case .array(let recipes)? = reply["recipes"],
              let recipe = recipes.first(where: { $0["name"]?.string == name }),
              let steps = recipe["steps"] else { return nil }
        return .object(["kind": .string("steps"), "steps": steps, "label": .string(name)])
    }

    /// Recipe names from the engine's `recipes` reply.
    public static func recipeNames(in reply: JSONValue) -> [String] {
        guard case .array(let recipes)? = reply["recipes"] else { return [] }
        return recipes.compactMap { $0["name"]?.string }
    }
}

/// One entry of a watched folder.
public struct WatchEntry: Equatable, Sendable {
    public let name: String
    public let size: Int64
    public let isDirectory: Bool

    public init(name: String, size: Int64, isDirectory: Bool) {
        self.name = name
        self.size = size
        self.isDirectory = isDirectory
    }
}

/// Decides which files in a watched folder are new and finished arriving.
/// Files present when watching starts are left alone; a new file is ready
/// once its size stays the same between two looks.
public struct WatchScanner: Sendable {
    private var seen: Set<String>
    private var pending: [String: (size: Int64, looks: Int)] = [:]
    /// How long an empty file is watched before giving up on it.
    static let emptyLooks = 20

    public init(existing: [String]) {
        seen = Set(existing)
    }

    /// True while some new file may still be arriving; look again soon.
    public var isWaiting: Bool {
        pending.values.contains { $0.size > 0 || $0.looks < Self.emptyLooks }
    }

    /// Files to process now.
    public mutating func scan(_ entries: [WatchEntry]) -> [String] {
        let present = Set(entries.map(\.name))
        // A file that was removed and added again counts as new.
        seen.formIntersection(present)
        pending = pending.filter { present.contains($0.key) }
        var ready: [String] = []
        for e in entries where !e.isDirectory && !Self.ignored(e.name) && !seen.contains(e.name) {
            if let last = pending[e.name], last.size == e.size, e.size > 0 {
                ready.append(e.name)
                seen.insert(e.name)
                pending.removeValue(forKey: e.name)
            } else {
                let looks = pending[e.name].map { $0.size == e.size ? $0.looks + 1 : 1 } ?? 1
                pending[e.name] = (e.size, looks)
            }
        }
        return ready.sorted()
    }

    /// Hidden files, Veer's own temporary files and downloads in progress.
    public static func ignored(_ name: String) -> Bool {
        if name.hasPrefix(".") || name.hasPrefix("~$") { return true }
        let partial: Set<String> = ["crdownload", "download", "part", "partial", "tmp", "opdownload"]
        let ext = (name as NSString).pathExtension.lowercased()
        return partial.contains(ext)
    }
}
