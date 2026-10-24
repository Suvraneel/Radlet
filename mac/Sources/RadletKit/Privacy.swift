import Foundation

/// One thing a file reveals, from the engine's `privacy` method.
public struct PrivacyFinding: Decodable, Equatable, Identifiable, Sendable {
    /// location, identity, device, time, software, hidden or other.
    public let kind: String
    public let label: String
    public let detail: String
    /// Whether Remove metadata takes it out.
    public let removable: Bool
    public let latitude: Double?
    public let longitude: Double?

    public var id: String { "\(kind)|\(label)|\(detail)" }

    public init(kind: String, label: String, detail: String, removable: Bool,
                latitude: Double? = nil, longitude: Double? = nil) {
        self.kind = kind
        self.label = label
        self.detail = detail
        self.removable = removable
        self.latitude = latitude
        self.longitude = longitude
    }

    /// The place in Apple Maps.
    public var mapsURL: URL? {
        guard let latitude, let longitude else { return nil }
        return URL(string: "https://maps.apple.com/?ll=\(latitude),\(longitude)&q=\(latitude),\(longitude)")
    }

    public var symbol: String {
        switch kind {
        case "location": return "location.fill"
        case "identity": return "person.crop.circle"
        case "device": return "camera"
        case "time": return "clock"
        case "software": return "hammer"
        case "hidden": return "eye.slash"
        default: return "text.bubble"
        }
    }
}

/// What one file reveals, or why it couldn't be checked.
public struct PrivacyReport: Decodable, Equatable, Identifiable, Sendable {
    public struct Problem: Decodable, Equatable, Sendable {
        public let code: String?
        public let message: String
    }

    public let path: String
    public let format: String?
    public let findings: [PrivacyFinding]
    public let error: Problem?

    public var id: String { path }
    public var name: String { URL(fileURLWithPath: path).lastPathComponent }

    public init(path: String, format: String?, findings: [PrivacyFinding], error: Problem? = nil) {
        self.path = path
        self.format = format
        self.findings = findings
        self.error = error
    }

    enum CodingKeys: String, CodingKey { case path, format, findings, error }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        path = try c.decode(String.self, forKey: .path)
        format = try c.decodeIfPresent(String.self, forKey: .format)
        findings = try c.decodeIfPresent([PrivacyFinding].self, forKey: .findings) ?? []
        error = try c.decodeIfPresent(Problem.self, forKey: .error)
    }

    /// Reports from the `privacy` result.
    public static func list(from result: JSONValue) throws -> [PrivacyReport] {
        guard let reports = result["reports"] else { return [] }
        return try reports.decode([PrivacyReport].self)
    }
}

/// The window's headline and what Remove metadata would do.
public enum PrivacySummary {
    static let order: [(kind: String, phrase: String)] = [
        ("location", "location"),
        ("identity", "names or serial numbers"),
        ("device", "camera or device"),
        ("time", "dates"),
        ("software", "apps used"),
        ("hidden", "hidden content"),
        ("other", "other details"),
    ]

    /// "Location in 2 of 3 files · names or serial numbers in 1 · …".
    public static func headline(_ reports: [PrivacyReport]) -> String {
        let checked = reports.filter { $0.error == nil }
        guard !checked.isEmpty else { return reports.isEmpty ? "" : "Radlet couldn't check these files." }
        var parts: [String] = []
        for (kind, phrase) in order {
            let n = checked.filter { r in r.findings.contains { $0.kind == kind } }.count
            guard n > 0 else { continue }
            if parts.isEmpty {
                let first = phrase.prefix(1).uppercased() + phrase.dropFirst()
                parts.append(checked.count == 1 ? first : "\(first) in \(n) of \(checked.count) files")
            } else {
                parts.append(checked.count == 1 ? phrase : "\(phrase) in \(n)")
            }
        }
        if parts.isEmpty { return checked.count == 1 ? "Nothing revealing found." : "Nothing revealing found in these files." }
        return parts.prefix(3).joined(separator: " · ") + (parts.count > 3 ? " · and more" : "")
    }

    /// Files Remove metadata would change.
    public static func filesToClean(_ reports: [PrivacyReport]) -> [String] {
        reports.filter { r in r.findings.contains(where: \.removable) }.map(\.path)
    }

    /// What stays after Remove metadata, by label, without repeats.
    public static func staying(_ reports: [PrivacyReport]) -> [String] {
        var seen: [String] = []
        for r in reports {
            for f in r.findings where !f.removable && !seen.contains(f.label) { seen.append(f.label) }
        }
        return seen
    }
}
