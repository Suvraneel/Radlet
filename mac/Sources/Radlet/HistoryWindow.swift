import AppKit
import SwiftUI
import RadletKit

/// Recent jobs, kept between launches, with Undo for each.
@MainActor
final class HistoryStore: ObservableObject {
    static let shared = HistoryStore()
    @Published private(set) var history: JobHistory

    private let file: URL = {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent("Library/Application Support")
        return base.appendingPathComponent("Radlet/history.json")
    }()

    private init() {
        history = JobHistory()
        history = JobHistory.decode(try? Data(contentsOf: file))
    }

    var nextUndo: HistoryEntry? { history.nextUndo }

    /// Records a finished job and returns its entry; jobs that changed
    /// nothing (a checksum, say) aren't listed.
    @discardableResult
    func record(title: String, outputs: [URL], restores: [HistoryEntry.Restore] = [],
                renames: [HistoryEntry.Rename] = []) -> UUID? {
        guard !(outputs.isEmpty && restores.isEmpty && renames.isEmpty) else { return nil }
        let entry = HistoryEntry(title: title, outputs: outputs.map(\.path), restores: restores, renames: renames)
        history.add(entry)
        save()
        return entry.id
    }

    func clear() {
        history.clear()
        save()
    }

    /// Reverses a job: renames go back, outputs go to the Trash, and
    /// replaced originals come back from it.
    func undo(_ id: UUID, completion: @escaping (String) -> Void) {
        guard let entry = history.entries.first(where: { $0.id == id }), entry.canUndo else {
            completion("Nothing to undo")
            return
        }
        history.markUndone(id)
        save()
        let fm = FileManager.default
        let renamedBack = Self.restoreNames(entry.renames)
        let outputs = entry.outputs.map { URL(fileURLWithPath: $0) }.filter { fm.fileExists(atPath: $0.path) }
        let finish: (Int) -> Void = { trashed in
            var restored = 0
            for r in entry.restores
            where fm.fileExists(atPath: r.trashed) && !fm.fileExists(atPath: r.original) {
                if (try? fm.moveItem(atPath: r.trashed, toPath: r.original)) != nil { restored += 1 }
            }
            var parts: [String] = []
            if renamedBack > 0 { parts.append("Put back \(renamedBack) name\(renamedBack == 1 ? "" : "s")") }
            if trashed > 0 { parts.append("Moved \(trashed) file\(trashed == 1 ? "" : "s") to the Trash") }
            if restored > 0 { parts.append("Put back \(restored) original\(restored == 1 ? "" : "s")") }
            Log.app("undo: \(renamedBack) names, \(trashed) outputs, \(restored) originals")
            completion(parts.isEmpty ? "Nothing left to undo" : parts.joined(separator: " · "))
        }
        guard !outputs.isEmpty else {
            finish(0)
            return
        }
        NSWorkspace.shared.recycle(outputs) { trashed, _ in
            DispatchQueue.main.async { MainActor.assumeIsolated { finish(trashed.count) } }
        }
    }

    /// Puts old names back in two steps (aside to hidden names, then to the
    /// old names), so names that were swapped or shifted ("1" to "2", "2"
    /// to "3") and case-only changes all come back. A name taken since
    /// gets a number.
    private static func restoreNames(_ renames: [HistoryEntry.Rename]) -> Int {
        let fm = FileManager.default
        var aside: [(temp: String, rename: HistoryEntry.Rename)] = []
        for r in renames where fm.fileExists(atPath: r.to) {
            let temp = (r.to as NSString).deletingLastPathComponent + "/.radlet-undo-\(UUID().uuidString)"
            do {
                try fm.moveItem(atPath: r.to, toPath: temp)
                aside.append((temp, r))
            } catch {
                Log.app("undo: couldn't move a renamed file: \(Log.code(error))")
            }
        }
        var count = 0
        for a in aside {
            let target = RenamePlan.freeName(URL(fileURLWithPath: a.rename.from)) {
                fm.fileExists(atPath: $0.path)
            }
            do {
                try fm.moveItem(atPath: a.temp, toPath: target.path)
                count += 1
            } catch {
                Log.app("undo: couldn't restore a name: \(Log.code(error))")
                let back = RenamePlan.freeName(URL(fileURLWithPath: a.rename.to)) { fm.fileExists(atPath: $0.path) }
                try? fm.moveItem(atPath: a.temp, toPath: back.path)
            }
        }
        return count
    }

    private func save() {
        try? FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? history.encoded()?.write(to: file, options: .atomic)
    }
}

@MainActor
final class HistoryWindowController {
    private var window: NSWindow?

    func show() {
        if window == nil {
            let host = NSHostingController(rootView: HistoryView(store: HistoryStore.shared))
            let w = NSWindow(contentViewController: host)
            w.title = "History"
            w.styleMask = [.titled, .closable, .miniaturizable, .resizable]
            w.isReleasedWhenClosed = false
            w.setContentSize(NSSize(width: 520, height: 460))
            w.center()
            window = w
        }
        NSApp.activate()
        window?.makeKeyAndOrderFront(nil)
    }
}

struct HistoryView: View {
    @ObservedObject var store: HistoryStore

    private static let relative: RelativeDateTimeFormatter = {
        let f = RelativeDateTimeFormatter()
        f.unitsStyle = .full
        return f
    }()

    var body: some View {
        VStack(spacing: 0) {
            if store.history.entries.isEmpty {
                Text("Jobs you run appear here, with Undo.")
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                List(store.history.entries) { entry in
                    HStack(alignment: .center, spacing: 10) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(entry.title).lineLimit(1).truncationMode(.middle)
                            Text("\(entry.summary) · \(Self.relative.localizedString(for: entry.date, relativeTo: Date()))")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        .opacity(entry.undone ? 0.5 : 1)
                        Spacer()
                        Button("Show") { reveal(entry) }
                            .disabled(revealable(entry).isEmpty)
                        Button("Undo") { AppState.shared.undo(entry.id) }
                        .disabled(!entry.canUndo)
                    }
                    .padding(.vertical, 3)
                }
            }
            Divider()
            HStack {
                Text("The last \(JobHistory.limit) jobs are kept on this Mac.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Spacer()
                Button("Clear History") { store.clear() }
                    .disabled(store.history.entries.isEmpty)
            }
            .padding(10)
        }
        .frame(minWidth: 420, minHeight: 300)
    }

    private func revealable(_ entry: HistoryEntry) -> [URL] {
        (entry.outputs + entry.renames.map(\.to))
            .map { URL(fileURLWithPath: $0) }
            .filter { FileManager.default.fileExists(atPath: $0.path) }
    }

    private func reveal(_ entry: HistoryEntry) {
        NSWorkspace.shared.activateFileViewerSelecting(revealable(entry))
    }
}
