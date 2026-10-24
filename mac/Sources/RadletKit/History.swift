import Foundation

/// One finished job, as History lists it and Undo reverses it.
public struct HistoryEntry: Codable, Identifiable, Equatable, Sendable {
    public struct Restore: Codable, Equatable, Sendable {
        /// Where the original went in the Trash.
        public let trashed: String
        public let original: String

        public init(trashed: String, original: String) {
            self.trashed = trashed
            self.original = original
        }
    }

    public struct Rename: Codable, Equatable, Sendable {
        public let from: String
        public let to: String

        public init(from: String, to: String) {
            self.from = from
            self.to = to
        }
    }

    public var id: UUID
    public var title: String
    public var date: Date
    /// Files and folders the job created.
    public var outputs: [String]
    /// Originals moved to the Trash by "Replace originals".
    public var restores: [Restore]
    /// Files renamed in place.
    public var renames: [Rename]
    public var undone: Bool

    public init(id: UUID = UUID(), title: String, date: Date = Date(), outputs: [String] = [],
                restores: [Restore] = [], renames: [Rename] = [], undone: Bool = false) {
        self.id = id
        self.title = title
        self.date = date
        self.outputs = outputs
        self.restores = restores
        self.renames = renames
        self.undone = undone
    }

    public var canUndo: Bool { !undone && !(outputs.isEmpty && restores.isEmpty && renames.isEmpty) }

    /// "3 files", "Renamed 12 files", "1 file · original in the Trash".
    public var summary: String {
        func files(_ n: Int) -> String { "\(n) file\(n == 1 ? "" : "s")" }
        var parts: [String] = []
        if !renames.isEmpty { parts.append("Renamed \(files(renames.count))") }
        if !outputs.isEmpty { parts.append(files(outputs.count)) }
        if !restores.isEmpty {
            parts.append("\(restores.count) original\(restores.count == 1 ? "" : "s") in the Trash")
        }
        if undone { parts.append("undone") }
        return parts.isEmpty ? "Nothing to undo" : parts.joined(separator: " · ")
    }
}

/// The most recent jobs, newest first, kept between launches.
public struct JobHistory: Codable, Equatable, Sendable {
    public static let limit = 50
    public private(set) var entries: [HistoryEntry] = []

    public init(entries: [HistoryEntry] = []) {
        self.entries = Array(entries.prefix(Self.limit))
    }

    public mutating func add(_ entry: HistoryEntry) {
        entries.insert(entry, at: 0)
        if entries.count > Self.limit { entries.removeLast(entries.count - Self.limit) }
    }

    /// What Undo reverses next: the newest job not yet undone.
    public var nextUndo: HistoryEntry? { entries.first { $0.canUndo } }

    public mutating func markUndone(_ id: UUID) {
        if let i = entries.firstIndex(where: { $0.id == id }) { entries[i].undone = true }
    }

    public mutating func clear() {
        entries.removeAll()
    }

    public static func decode(_ data: Data?) -> JobHistory {
        guard let data else { return JobHistory() }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return (try? decoder.decode(JobHistory.self, from: data)) ?? JobHistory()
    }

    public func encoded() -> Data? {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        return try? encoder.encode(self)
    }
}
