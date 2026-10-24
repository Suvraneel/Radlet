import AppKit
import RadletKit

/// Remembers which conversions you use, so they're the ones that stay on
/// the wheel when not everything fits.
@MainActor
final class Usage {
    static let shared = Usage()
    private let key = "slotUsage"
    private(set) var counts: [String: Int]

    private init() {
        counts = UserDefaults.standard.dictionary(forKey: key) as? [String: Int] ?? [:]
    }

    func record(_ slot: Slot) {
        counts[slot.id, default: 0] += 1
        UserDefaults.standard.set(counts, forKey: key)
    }

    func reset() {
        counts.removeAll()
        UserDefaults.standard.removeObject(forKey: key)
    }
}

/// "Replace originals": after a job, sends each original to the Trash and
/// gives a same-format result the original's name.
@MainActor
enum Replacer {
    struct Result {
        /// Output URLs that were renamed, old to new.
        var renamed: [URL: URL] = [:]
        var restores: [HistoryEntry.Restore] = []
    }

    static func run(_ items: [ReplaceCandidate], completion: @escaping (Result) -> Void) {
        let fm = FileManager.default
        let eligible = items.filter { c in
            var isDir: ObjCBool = false
            guard ReplacePlan.eligible(c), fm.fileExists(atPath: c.input.path),
                  fm.fileExists(atPath: c.output.path, isDirectory: &isDir), !isDir.boolValue else { return false }
            return true
        }
        guard !eligible.isEmpty else {
            completion(Result())
            return
        }
        NSWorkspace.shared.recycle(eligible.map(\.input)) { trashed, error in
            DispatchQueue.main.async {
                // Match by path: the keys may not be byte-for-byte the URLs
                // passed in.
                let byPath = Dictionary(trashed.map { ($0.key.standardizedFileURL.path, $0.value) },
                                        uniquingKeysWith: { first, _ in first })
                var result = Result()
                for c in eligible {
                    guard let inTrash = byPath[c.input.standardizedFileURL.path] else { continue }
                    result.restores.append(HistoryEntry.Restore(trashed: inTrash.path, original: c.input.path))
                    guard let target = ReplacePlan.renameTarget(c) else { continue }
                    do {
                        try fm.moveItem(at: c.output, to: target)
                        result.renamed[c.output] = target
                    } catch {
                        Log.app("replace: couldn't rename a result: \(Log.code(error))")
                    }
                }
                if let error { Log.app("replace: \(Log.code(error))") }
                completion(result)
            }
        }
    }
}
