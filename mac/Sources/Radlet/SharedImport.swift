import AppKit
import SwiftUI
import UniformTypeIdentifiers
import RadletKit
import RadletUI

/// Opens the loadouts and skins people share, as files or radlet:// links
/// (the website's gallery uses links). Each opens a window that shows what
/// it holds; nothing is added, and nothing ever runs, until asked.
@MainActor
final class SharedImport: NSObject, NSWindowDelegate {
    static let shared = SharedImport()
    private var open: [(window: NSWindow, model: ImportModel)] = []

    /// Opens every link and shared file in `urls`, and returns the rest
    /// (files for the wheel).
    func take(_ urls: [URL]) -> [URL] {
        var rest: [URL] = []
        for url in urls {
            if url.scheme?.lowercased() == SharedItem.scheme {
                Log.app("shared link opened")
                show(SharedItem.parse(link: url))
            } else if url.isFileURL, SharedItem.kind(ofFile: url) != nil {
                Log.app("shared file opened: \(url.pathExtension)")
                show(SharedItem.read(file: url))
            } else {
                rest.append(url)
            }
        }
        return rest
    }

    private func show(_ result: Result<SharedItem, ShareProblem>) {
        let model = ImportModel(result)
        let host = NSHostingController(rootView: ImportView(model: model))
        let w = NSWindow(contentViewController: host)
        w.title = "Radlet"
        w.styleMask = [.titled, .closable]
        w.isReleasedWhenClosed = false
        w.delegate = self
        model.close = { [weak w] in w?.close() }
        if let last = open.last?.window {
            w.setFrameTopLeftPoint(NSPoint(x: last.frame.minX + 24, y: last.frame.maxY - 24))
        } else {
            w.center()
        }
        open.append((w, model))
        NSApp.activate()
        w.makeKeyAndOrderFront(nil)
        model.load()
    }

    func windowWillClose(_ notification: Notification) {
        guard let closing = notification.object as? NSWindow else { return }
        if let i = open.firstIndex(where: { $0.window === closing }) {
            open[i].model.preview.stop()
            open.remove(at: i)
        }
    }
}

/// What one import window shows.
@MainActor
final class ImportModel: ObservableObject {
    enum Phase: Equatable {
        case loading, ready, adding
        case failed(String)
    }

    @Published private(set) var phase: Phase
    let kind: SharedItem.Kind?
    private let item: SharedItem?

    /// The name it's added under, and whether that differs from the one
    /// it came with (because one by that name is already here).
    @Published private(set) var name = ""
    @Published private(set) var renamed = false
    // A loadout: its description and steps, in words.
    @Published private(set) var summary = ""
    @Published private(set) var steps: [String] = []
    private var recipe: [String: JSONValue] = [:]
    // A skin: as read, with any lines left out.
    @Published private(set) var author = ""
    @Published private(set) var issues: [SkinIssue] = []
    private var skin: Skin?
    @Published var useNow = true
    let preview = WheelPreviewModel()

    var close: () -> Void = {}

    init(_ result: Result<SharedItem, ShareProblem>) {
        switch result {
        case .success(let item):
            self.item = item
            kind = item.kind
            phase = .loading
        case .failure(let problem):
            item = nil
            kind = nil
            phase = .failed(problem.message)
        }
    }

    var title: String {
        switch kind {
        case .loadout: return "Add this loadout?"
        case .skin: return "Add this skin?"
        case nil: return "Can't open this"
        }
    }

    var symbol: String {
        switch kind {
        case .loadout: return "square.stack.3d.up"
        case .skin: return "paintpalette"
        case nil: return "exclamationmark.triangle"
        }
    }

    func load() {
        guard let item, phase == .loading else { return }
        switch item.kind {
        case .loadout:
            Task { await loadRecipe(item.text) }
        case .skin:
            let parsed = SkinCSS.parse(item.text, id: "shared", fallbackName: "Shared skin",
                                       bases: SkinLibrary.builtIns.map(\.skin))
            var s = parsed.skin
            let taken = SkinStore.shared.files.map(\.skin.name)
            let free = ImportNaming.free(s.name, taken: taken)
            renamed = free != s.name
            s.name = free
            skin = s
            name = free
            author = s.author
            issues = parsed.issues
            preview.update(style: .dial, count: WheelSpec.defaultCount, skin: s)
            preview.start()
            phase = .ready
        }
    }

    private func loadRecipe(_ text: String) async {
        do {
            let result = try await EngineClient.shared.request("parse_recipe", ["text": .string(text)])
            guard case .object(var r)? = result["recipe"] else {
                phase = .failed("This loadout couldn't be read.")
                return
            }
            let listing = try await EngineClient.shared.request(
                "recipes", ["recipes_dir": .string(Prefs.loadoutsDir.path)])
            let taken = RecipeListing(listing).recipes.map(\.name)
            let original = r["name"]?.string ?? "Loadout"
            let free = ImportNaming.free(original, taken: taken)
            r["name"] = .string(free)
            recipe = r
            name = free
            renamed = free != original
            summary = r["description"]?.string ?? ""
            if case .array(let list)? = r["steps"] { steps = list.map(StepText.describe) }
            phase = .ready
        } catch {
            phase = .failed("This loadout can't be added: \(error.localizedDescription)")
        }
    }

    func add() {
        guard phase == .ready, let kind else { return }
        phase = .adding
        switch kind {
        case .loadout:
            let params: [String: JSONValue] = ["recipe": .object(recipe),
                                               "recipes_dir": .string(Prefs.loadoutsDir.path)]
            Task {
                do {
                    _ = try await EngineClient.shared.request("save_recipe", params)
                    Log.app("shared loadout added")
                    AppState.shared.notify("Loadout added",
                                           detail: "\u{201C}\(name)\u{201D} is on the Control wheel")
                    close()
                } catch {
                    phase = .failed("The loadout couldn't be saved: \(error.localizedDescription)")
                }
            }
        case .skin:
            guard let skin else { return }
            do {
                try FileManager.default.createDirectory(at: Prefs.skinsDir, withIntermediateDirectories: true)
                let url = SkinLibrary.newFileURL(in: Prefs.skinsDir, name: skin.name)
                // Written afresh: only Radlet's own tokens, under the name shown.
                try skin.compactCSS().write(to: url, atomically: true, encoding: .utf8)
                let store = SkinStore.shared
                store.reload()
                if useNow { store.selectedID = "file:\(url.lastPathComponent)" }
                Log.app("shared skin added")
                AppState.shared.notify("Skin added", detail: useNow
                                       ? "The wheel now wears \u{201C}\(name)\u{201D}"
                                       : "\u{201C}\(name)\u{201D} is in Settings \u{2192} Wheel")
                close()
            } catch {
                phase = .failed("The skin couldn't be saved: \(Log.code(error))")
            }
        }
    }
}

struct ImportView: View {
    @ObservedObject var model: ImportModel

    var body: some View {
        VStack(spacing: 0) {
            HStack(alignment: .top, spacing: 14) {
                Image(systemName: model.symbol)
                    .font(.system(size: 28, weight: .light))
                    .foregroundStyle(.tint)
                    .frame(width: 36)
                VStack(alignment: .leading, spacing: 10) {
                    Text(model.title).font(.headline)
                    content
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(20)
            Divider()
            HStack(spacing: 10) {
                if model.kind == .skin, model.phase == .ready {
                    Toggle("Use it now", isOn: $model.useNow)
                }
                Spacer()
                Button(model.kind == nil ? "OK" : "Cancel") { model.close() }
                    .keyboardShortcut(.cancelAction)
                if let kind = model.kind {
                    Button("Add \(kind.title)") { model.add() }
                        .keyboardShortcut(.defaultAction)
                        .disabled(model.phase != .ready)
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
        }
        .frame(width: model.kind == .skin ? 520 : 460)
    }

    @ViewBuilder private var content: some View {
        switch model.phase {
        case .loading:
            ProgressView().controlSize(.small)
        case .failed(let message):
            Text(message)
                .foregroundStyle(.red)
                .fixedSize(horizontal: false, vertical: true)
        case .ready, .adding:
            if model.kind == .skin { skin } else { loadout }
        }
    }

    private var loadout: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(model.name).font(.title3.weight(.semibold))
            if model.renamed {
                note("You already have a loadout by that name, so this one is added as \u{201C}\(model.name)\u{201D}.")
            }
            if !model.summary.isEmpty {
                Text(model.summary).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            VStack(alignment: .leading, spacing: 3) {
                ForEach(Array(model.steps.enumerated()), id: \.offset) { i, step in
                    Text("\(i + 1).  \(step)")
                }
            }
            .padding(.vertical, 4)
            note("It joins the Control wheel. Nothing runs until you choose it there, and its results are new files beside your originals.")
        }
    }

    private var skin: some View {
        HStack(alignment: .top, spacing: 16) {
            WheelPreview(model: model.preview.model)
                .scaleEffect(170.0 / 236.0)
                .frame(width: 170, height: 170)
            VStack(alignment: .leading, spacing: 8) {
                Text(model.name).font(.title3.weight(.semibold))
                if !model.author.isEmpty {
                    Text("by \(model.author)").foregroundStyle(.secondary)
                }
                if model.renamed {
                    note("You already have a skin by that name, so this one is added as \u{201C}\(model.name)\u{201D}.")
                }
                if !model.issues.isEmpty {
                    note(model.issues.count == 1
                         ? "One line isn't something Radlet understands, so it's left out."
                         : "\(model.issues.count) lines aren't things Radlet understands, so they're left out.")
                }
                note("A skin only changes how the wheel looks. Switch back any time in Settings \u{2192} Wheel.")
            }
        }
    }

    private func note(_ text: String) -> some View {
        Text(text)
            .font(.callout)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
    }
}

/// Sharing a loadout or skin: as a link on the clipboard, or as a file.
@MainActor
enum ShareExport {
    static func copyLink(_ item: SharedItem, name: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(item.link.absoluteString, forType: .string)
        Log.app("\(item.kind.rawValue) link copied")
        AppState.shared.notify("Link copied", detail: "Anyone with Radlet can open it to add \u{201C}\(name)\u{201D}")
    }

    static func saveFile(_ item: SharedItem, name: String) {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = item.fileName(for: name)
        if let type = UTType(filenameExtension: item.kind.fileExtension) {
            panel.allowedContentTypes = [type]
        }
        panel.canCreateDirectories = true
        NSApp.activate()
        guard panel.runModal() == .OK, var url = panel.url else { return }
        if url.pathExtension.lowercased() != item.kind.fileExtension {
            url.appendPathExtension(item.kind.fileExtension)
        }
        do {
            try item.text.write(to: url, atomically: true, encoding: .utf8)
            Log.app("\(item.kind.rawValue) saved as a file")
        } catch {
            let alert = NSAlert()
            alert.messageText = "The file couldn't be saved."
            alert.informativeText = error.localizedDescription
            alert.runModal()
        }
    }
}
