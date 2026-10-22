import Foundation

/// A finished item that made exactly one new file from one original.
public struct ReplaceCandidate: Equatable, Sendable {
    public let input: URL
    public let output: URL

    public init(input: URL, output: URL) {
        self.input = input
        self.output = output
    }
}

/// Rules for "Replace originals": the original goes to the Trash and, when
/// the format didn't change, the new file takes the original's name.
public enum ReplacePlan {
    /// Only files saved next to their original are replaced; a copy sent
    /// to another folder leaves the original where it is.
    public static func eligible(_ c: ReplaceCandidate) -> Bool {
        let a = c.input.standardizedFileURL, b = c.output.standardizedFileURL
        return a != b && a.deletingLastPathComponent() == b.deletingLastPathComponent()
    }

    /// Where the new file should move once the original is in the Trash:
    /// the original's own name when the extension is the same ("photo
    /// compressed.jpg" becomes "photo.jpg"), otherwise nowhere ("photo.png"
    /// converted to "photo.jpg" already has the best name).
    public static func renameTarget(_ c: ReplaceCandidate) -> URL? {
        guard eligible(c),
              c.input.pathExtension.lowercased() == c.output.pathExtension.lowercased() else { return nil }
        return c.input
    }
}
