import Foundation

/// Why a shared loadout or skin can't be opened, in words for the window.
public struct ShareProblem: Error, Equatable, Sendable {
    public let message: String

    public init(_ message: String) {
        self.message = message
    }
}

/// A loadout or skin someone shared: as a file that opens in Radlet, or as
/// a radlet:// link, which is what the website's gallery uses.
///
///     radlet://add/loadout?v=1&data=<the loadout's TOML, base64url>
///     radlet://add/skin?v=1&data=<the skin's CSS, base64url>
///
/// Opening one never runs or changes anything by itself: Radlet shows what
/// it holds and adds it only when asked.
public struct SharedItem: Equatable, Sendable {
    public enum Kind: String, CaseIterable, Sendable {
        case loadout, skin

        /// A file of this kind: the loadouts folder's TOML, or a skin's CSS.
        public var fileExtension: String { self == .loadout ? "radletloadout" : "radletskin" }

        /// The type the app declares for these files.
        public var typeIdentifier: String { "com.suvraneel.radlet.\(rawValue)" }

        /// "Loadout", "Skin".
        public var title: String { rawValue.capitalized }

        /// Far more than Radlet ever writes: 16 steps, or every skin token.
        public var maxBytes: Int { self == .loadout ? 16 * 1024 : SkinLibrary.maxBytes }
    }

    public static let scheme = "radlet"
    /// The link format this Radlet writes and reads.
    public static let version = "1"

    public let kind: Kind
    public let text: String

    /// Nil for empty text or text over the kind's size limit.
    public init?(kind: Kind, text: String) {
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              text.utf8.count <= kind.maxBytes else { return nil }
        self.kind = kind
        self.text = text
    }

    /// The radlet:// link that opens this in Radlet.
    public var link: URL {
        var c = URLComponents()
        c.scheme = Self.scheme
        c.host = "add"
        c.path = "/\(kind.rawValue)"
        c.queryItems = [URLQueryItem(name: "v", value: Self.version),
                        URLQueryItem(name: "data", value: Self.base64URL(Data(text.utf8)))]
        // Every part is plain ASCII, so this can't fail.
        return c.url!
    }

    /// Reads a radlet:// link.
    public static func parse(link url: URL) -> Result<SharedItem, ShareProblem> {
        guard url.scheme?.lowercased() == scheme,
              let c = URLComponents(url: url, resolvingAgainstBaseURL: false),
              c.host?.lowercased() == "add" else {
            return .failure(ShareProblem("This isn't a link Radlet knows."))
        }
        let name = c.path.trimmingCharacters(in: CharacterSet(charactersIn: "/")).lowercased()
        guard let kind = Kind(rawValue: name) else {
            return .failure(ShareProblem("This link is for something this version of Radlet doesn't have. Check for updates."))
        }
        let items = c.queryItems ?? []
        if let v = items.first(where: { $0.name == "v" })?.value, v != version {
            return .failure(ShareProblem("This link was made by a newer Radlet. Check for updates, then open it again."))
        }
        guard let encoded = items.first(where: { $0.name == "data" })?.value,
              encoded.count <= (kind.maxBytes * 4) / 3 + 4,
              let data = decodeBase64URL(encoded),
              let text = String(data: data, encoding: .utf8) else {
            return .failure(ShareProblem("This link is incomplete or damaged. Try copying it again."))
        }
        guard let item = SharedItem(kind: kind, text: text) else {
            return .failure(ShareProblem("This \(kind.rawValue) is empty or too big to be one Radlet made."))
        }
        return .success(item)
    }

    /// The kind of shared file at `url`, by its extension.
    public static func kind(ofFile url: URL) -> Kind? {
        let ext = url.pathExtension.lowercased()
        return Kind.allCases.first { $0.fileExtension == ext }
    }

    /// Reads a shared file.
    public static func read(file url: URL) -> Result<SharedItem, ShareProblem> {
        guard let kind = kind(ofFile: url) else {
            return .failure(ShareProblem("This isn't a loadout or skin file."))
        }
        let size = (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0
        guard size <= kind.maxBytes, let data = try? Data(contentsOf: url),
              let text = String(data: data, encoding: .utf8) else {
            return .failure(ShareProblem("This \(kind.rawValue) file can't be read, or is too big to be one Radlet made."))
        }
        guard let item = SharedItem(kind: kind, text: text) else {
            return .failure(ShareProblem("This \(kind.rawValue) file is empty."))
        }
        return .success(item)
    }

    /// A file name for sharing `name` as this kind: "Web-ready image.radletloadout".
    public func fileName(for name: String) -> String {
        let cleaned = String(name.map { "/:\\".contains($0) || $0.isNewline ? "-" : $0 })
            .trimmingCharacters(in: .whitespaces)
        let stem = cleaned.isEmpty || cleaned.hasPrefix(".") ? kind.title : String(cleaned.prefix(60))
        return "\(stem).\(kind.fileExtension)"
    }

    static func base64URL(_ data: Data) -> String {
        data.base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }

    static func decodeBase64URL(_ s: String) -> Data? {
        var b = s.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        // Links may arrive with padding, or with spaces where + was.
        b = b.replacingOccurrences(of: " ", with: "+")
        while b.count % 4 != 0 { b += "=" }
        return Data(base64Encoded: b)
    }
}

/// Names for things added from outside, so they don't clash with the
/// ones already there: "Web-ready image 2".
public enum ImportNaming {
    public static func free(_ name: String, taken: [String]) -> String {
        let base = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let used = Set(taken.map { $0.lowercased() })
        guard used.contains(base.lowercased()) else { return base }
        for n in 2...999 where !used.contains("\(base) \(n)".lowercased()) {
            return "\(base) \(n)"
        }
        return "\(base) copy"
    }
}
