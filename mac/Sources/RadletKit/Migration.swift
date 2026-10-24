import Foundation

/// Radlet was called Veer in earlier builds. Each launch, while the old
/// folder is still there, the app carries that build's files over to
/// Radlet's places: loadouts (then called recipes), skins and job history.
/// Settings are copied by the app (see LegacySettings in the Radlet target).
public enum LegacyFiles {
    /// The bundle ID the earlier builds used, for their settings.
    public static let oldBundleID = "com.usemonoma.veer"

    /// Each old item under Application Support and where it now lives.
    public static func moves(support: URL) -> [(from: URL, to: URL)] {
        let old = support.appendingPathComponent("Veer", isDirectory: true)
        let new = support.appendingPathComponent("Radlet", isDirectory: true)
        return [
            (old.appendingPathComponent("recipes", isDirectory: true),
             new.appendingPathComponent("Loadouts", isDirectory: true)),
            (old.appendingPathComponent("Skins", isDirectory: true),
             new.appendingPathComponent("Skins", isDirectory: true)),
            (old.appendingPathComponent("history.json"), new.appendingPathComponent("history.json")),
        ]
    }

    /// Moves every old item that has no counterpart yet. Inside folders
    /// that exist in both places, moves only the files the new one lacks.
    /// Nothing is overwritten or deleted, except the old folder itself
    /// once it's empty. Returns how many items moved.
    @discardableResult
    public static func migrate(support: URL, fileManager fm: FileManager = .default) -> Int {
        var moved = 0
        for (from, to) in moves(support: support) where fm.fileExists(atPath: from.path) {
            moved += move(from, to, fm)
        }
        let old = support.appendingPathComponent("Veer", isDirectory: true)
        if let left = try? fm.contentsOfDirectory(atPath: old.path), left.allSatisfy({ $0 == ".DS_Store" }) {
            try? fm.removeItem(at: old)
        }
        return moved
    }

    private static func move(_ from: URL, _ to: URL, _ fm: FileManager) -> Int {
        var isDir: ObjCBool = false
        guard fm.fileExists(atPath: to.path, isDirectory: &isDir) else {
            try? fm.createDirectory(at: to.deletingLastPathComponent(), withIntermediateDirectories: true)
            return (try? fm.moveItem(at: from, to: to)) != nil ? 1 : 0
        }
        // Both exist: only folders merge, file by file.
        var fromIsDir: ObjCBool = false
        guard isDir.boolValue, fm.fileExists(atPath: from.path, isDirectory: &fromIsDir), fromIsDir.boolValue,
              let children = try? fm.contentsOfDirectory(atPath: from.path) else { return 0 }
        var moved = 0
        for name in children where name != ".DS_Store" {
            moved += move(from.appendingPathComponent(name), to.appendingPathComponent(name), fm)
        }
        if (try? fm.contentsOfDirectory(atPath: from.path))?.allSatisfy({ $0 == ".DS_Store" }) ?? false {
            try? fm.removeItem(at: from)
        }
        return moved
    }

    /// Settings worth carrying over: the app's own keys, not the system's
    /// (window positions and the like, which start with NS or Apple).
    public static func carriesOver(_ key: String) -> Bool {
        !key.hasPrefix("NS") && !key.hasPrefix("Apple") && !key.hasPrefix("com.apple")
    }
}
