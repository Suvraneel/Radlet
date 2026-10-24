import AppKit
import SwiftUI
import RadletKit

/// Watches folders and quietly runs each one's task on new files. Results
/// go into a "Converted" folder inside; originals are never touched.
@MainActor
final class WatchManager: ObservableObject {
    static let shared = WatchManager()
    private static let key = "watchRules"

    @Published private(set) var rules: [WatchRule]
    /// A short line per rule: watching, last results, or a problem.
    @Published private(set) var status: [UUID: String] = [:]
    @Published private(set) var recipeNames: [String] = []
    private var watchers: [UUID: FolderWatcher] = [:]

    private init() {
        rules = WatchRule.decodeList(UserDefaults.standard.data(forKey: Self.key))
    }

    func start() {
        restart()
    }

    func add() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        panel.prompt = "Watch"
        panel.message = "Choose a folder. Radlet handles each new file added to it."
        guard panel.runModal() == .OK, let url = panel.url,
              !rules.contains(where: { $0.path == url.path }) else { return }
        // Starts paused, so nothing runs before you've chosen what to do.
        update { $0.append(WatchRule(path: url.path, task: .preset("to_jpeg"), enabled: false)) }
    }

    func remove(_ id: UUID) {
        update { $0.removeAll { $0.id == id } }
    }

    func setEnabled(_ id: UUID, _ on: Bool) {
        update { rules in
            if let i = rules.firstIndex(where: { $0.id == id }) { rules[i].enabled = on }
        }
    }

    func setTask(_ id: UUID, _ task: WatchTask) {
        update { rules in
            if let i = rules.firstIndex(where: { $0.id == id }) { rules[i].task = task }
        }
    }

    func reveal(_ rule: WatchRule) {
        let converted = rule.outputURL
        if FileManager.default.fileExists(atPath: converted.path) {
            NSWorkspace.shared.activateFileViewerSelecting([converted])
        } else {
            NSWorkspace.shared.open(rule.url)
        }
    }

    /// Settings opened: try again for folders that weren't there before.
    func retryMissing() {
        restart()
    }

    func refreshRecipes() async {
        let reply = try? await EngineClient.shared.request("recipes", ["recipes_dir": .string(Prefs.loadoutsDir.path)])
        recipeNames = reply.map { WatchPresets.recipeNames(in: $0) } ?? []
    }

    private func update(_ change: (inout [WatchRule]) -> Void) {
        var next = rules
        change(&next)
        guard next != rules else { return }
        rules = next
        UserDefaults.standard.set(WatchRule.encodeList(rules), forKey: Self.key)
        restart()
    }

    /// Starts and stops watchers to match the rules.
    private func restart() {
        for (id, watcher) in watchers {
            if let r = rules.first(where: { $0.id == id }), r.enabled, r.path == watcher.folder.path { continue }
            watcher.stop()
            watchers[id] = nil
        }
        for rule in rules {
            guard rule.enabled else {
                status[rule.id] = "Paused. Choose a task, then tick the box to start."
                continue
            }
            guard watchers[rule.id] == nil else { continue }
            let w = FolderWatcher(id: rule.id, folder: rule.url)
            w.onReady = { [weak self] id, files in self?.process(id, files) }
            w.onGone = { [weak self] id in
                self?.watchers[id]?.stop()
                self?.watchers[id] = nil
                self?.status[id] = "The folder was moved or deleted."
                Log.app("watch: a folder disappeared")
            }
            if w.start() {
                watchers[rule.id] = w
                status[rule.id] = "Watching"
            } else {
                status[rule.id] = "Folder not found."
            }
        }
        Log.app("watching \(watchers.count) folder\(watchers.count == 1 ? "" : "s")")
    }

    private func process(_ id: UUID, _ files: [URL]) {
        guard let rule = rules.first(where: { $0.id == id }), rule.enabled else { return }
        Log.app("watch: \(rule.task.title) on \(describeFiles(files))")
        status[id] = "Working on \(files.count) file\(files.count == 1 ? "" : "s")…"
        let tally = Tally()
        Task { [self] in
            guard let resolved = await action(for: rule.task) else {
                status[id] = "That loadout isn't there any more (renamed or deleted). Choose another task."
                return
            }
            try? FileManager.default.createDirectory(at: rule.outputURL, withIntermediateDirectories: true)
            let output: JSONValue = .object(["kind": .string("directory"), "path": .string(rule.outputURL.path)])
            do {
                _ = try await EngineClient.shared.run(files: files, action: resolved, output: output, isolate: true) { [self] event in
                    switch event {
                    case .finished(let o):
                        switch o.status {
                        case "done": tally.done += 1
                        case "skipped": tally.skipped += 1
                        default: tally.failed += 1
                        }
                    case .jobFinished:
                        status[id] = tally.summary
                        Log.app("watch: \(tally.summary)")
                    default:
                        break
                    }
                }
            } catch {
                status[id] = error.localizedDescription
            }
        }
    }

    private func action(for task: WatchTask) async -> JSONValue? {
        switch task {
        case .preset(let id):
            return WatchPresets.find(id)?.action
        case .recipe(let name):
            let reply = try? await EngineClient.shared.request("recipes", ["recipes_dir": .string(Prefs.loadoutsDir.path)])
            return reply.flatMap { WatchPresets.recipeAction(named: name, in: $0) }
        }
    }

    @MainActor
    private final class Tally {
        var done = 0, skipped = 0, failed = 0

        var summary: String {
            var parts: [String] = []
            if done > 0 { parts.append("\(done) done") }
            if skipped > 0 { parts.append("\(skipped) skipped") }
            if failed > 0 { parts.append("\(failed) failed") }
            let time = Date().formatted(date: .omitted, time: .shortened)
            return "Last run \(time): " + (parts.isEmpty ? "nothing to do" : parts.joined(separator: ", "))
        }
    }
}

/// Watches one folder's entries (not its subfolders) and reports new
/// files once they've finished arriving.
@MainActor
final class FolderWatcher {
    let id: UUID
    let folder: URL
    var onReady: ((UUID, [URL]) -> Void)?
    var onGone: ((UUID) -> Void)?
    private var source: DispatchSourceFileSystemObject?
    private var scanner = WatchScanner(existing: [])
    private var pendingScan: Task<Void, Never>?

    init(id: UUID, folder: URL) {
        self.id = id
        self.folder = folder
    }

    func start() -> Bool {
        let fd = open(folder.path, O_EVTONLY)
        guard fd >= 0 else { return false }
        // Files already there are left alone.
        guard let existing = entries() else {
            Darwin.close(fd)
            return false
        }
        scanner = WatchScanner(existing: existing.map(\.name))
        let src = DispatchSource.makeFileSystemObjectSource(fileDescriptor: fd, eventMask: [.write, .delete, .rename],
                                                            queue: .main)
        src.setEventHandler { [weak self] in
            MainActor.assumeIsolated { self?.changed() }
        }
        src.setCancelHandler { Darwin.close(fd) }
        src.resume()
        source = src
        return true
    }

    func stop() {
        pendingScan?.cancel()
        source?.cancel()
        source = nil
    }

    private func changed() {
        guard let flags = source?.data else { return }
        if flags.contains(.delete) || flags.contains(.rename) {
            onGone?(id)
            return
        }
        scheduleScan(after: 1)
    }

    private func scheduleScan(after seconds: Double) {
        pendingScan?.cancel()
        pendingScan = Task { [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
            guard !Task.isCancelled else { return }
            self?.scan()
        }
    }

    private func scan() {
        // A failed listing (a network volume hiccup) must not look like an
        // empty folder, or every file would seem new afterwards.
        guard let list = entries() else {
            scheduleScan(after: 5)
            return
        }
        let ready = scanner.scan(list)
        if !ready.isEmpty { onReady?(id, ready.map { folder.appendingPathComponent($0) }) }
        // Files still copying (or not yet written) don't send more events.
        if scanner.isWaiting { scheduleScan(after: 1.5) }
    }

    private func entries() -> [WatchEntry]? {
        let keys: [URLResourceKey] = [.isDirectoryKey, .fileSizeKey]
        guard let urls = try? FileManager.default.contentsOfDirectory(at: folder, includingPropertiesForKeys: keys) else {
            return nil
        }
        return urls.map { url in
            let v = try? url.resourceValues(forKeys: Set(keys))
            return WatchEntry(name: url.lastPathComponent, size: Int64(v?.fileSize ?? 0),
                              isDirectory: v?.isDirectory ?? false)
        }
    }
}

/// Settings → Watch.
struct WatchSettings: View {
    @ObservedObject private var watch = WatchManager.shared

    var body: some View {
        Form {
            Section {
                Text("Files added to these folders are handled automatically. Results go into a \"Converted\" folder inside; the originals stay as they are.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Section("Folders") {
                if watch.rules.isEmpty {
                    Text("No folders yet.").foregroundStyle(.secondary)
                }
                ForEach(watch.rules) { rule in
                    WatchRow(rule: rule, recipes: watch.recipeNames, status: watch.status[rule.id], watch: watch)
                }
                Button("Add Folder…") { watch.add() }
            }
        }
        .formStyle(.grouped)
        .onAppear { watch.retryMissing() }
        .task { await watch.refreshRecipes() }
    }
}

private struct WatchRow: View {
    let rule: WatchRule
    let recipes: [String]
    let status: String?
    let watch: WatchManager

    private var recipeChoices: [String] {
        var names = recipes
        if case .recipe(let current) = rule.task, !names.contains(current) { names.append(current) }
        return names
    }

    var body: some View {
        HStack(spacing: 10) {
            Button { watch.setEnabled(rule.id, !rule.enabled) } label: {
                Image(systemName: rule.enabled ? "checkmark.square.fill" : "square")
                    .foregroundStyle(rule.enabled ? Color.accentColor : Color.secondary)
            }
            .buttonStyle(.borderless)
            .help(rule.enabled ? "Pause" : "Resume")
            VStack(alignment: .leading, spacing: 2) {
                Text(rule.url.lastPathComponent).lineLimit(1).help(rule.path)
                if let status {
                    Text(status).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            Spacer()
            Menu(rule.task.title) {
                ForEach(WatchPresets.all, id: \.id) { p in
                    Button(p.title) { watch.setTask(rule.id, .preset(p.id)) }
                }
                if !recipeChoices.isEmpty {
                    Divider()
                    ForEach(recipeChoices, id: \.self) { name in
                        Button("Loadout: \(name)") { watch.setTask(rule.id, .recipe(name)) }
                    }
                }
            }
            .frame(width: 190)
            Button { watch.reveal(rule) } label: { Image(systemName: "magnifyingglass") }
                .buttonStyle(.borderless)
                .help("Show in Finder")
            Button { watch.remove(rule.id) } label: { Image(systemName: "minus.circle") }
                .buttonStyle(.borderless)
                .help("Stop watching this folder")
        }
    }
}
