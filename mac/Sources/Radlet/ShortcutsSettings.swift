import AppKit
import SwiftUI
import RadletKit

/// Which of the person's shortcuts (from the Shortcuts app) sit on the
/// Control wheel, after their loadouts. Each file goes in as the
/// shortcut's input; what it returns is saved beside the file.
@MainActor
final class ShortcutChoices: ObservableObject {
    @Published private(set) var available: [String] = []
    @Published private(set) var loading = true
    @Published private(set) var problem: String?
    @Published private(set) var chosen: [String] = Prefs.wheelShortcuts

    /// Chosen shortcuts that the Shortcuts app no longer has.
    var missing: [String] { chosen.filter { !available.contains($0) } }

    func load() {
        Task {
            do {
                let result = try await EngineClient.shared.request("shortcuts")
                if case .array(let names)? = result["shortcuts"] {
                    available = names.compactMap(\.string)
                }
                problem = nil
            } catch {
                problem = error.localizedDescription
            }
            loading = false
        }
    }

    func isOn(_ name: String) -> Bool { chosen.contains(name) }

    func set(_ name: String, on: Bool) {
        if on {
            guard !chosen.contains(name) else { return }
            chosen.append(name)
        } else {
            chosen.removeAll { $0 == name }
        }
        Prefs.wheelShortcuts = chosen
        Log.app("shortcuts on the wheel: \(chosen.count)")
    }

    func openShortcutsApp() {
        if let app = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.shortcuts") {
            NSWorkspace.shared.openApplication(at: app, configuration: NSWorkspace.OpenConfiguration())
        }
    }
}

struct ShortcutsPicker: View {
    @StateObject private var model = ShortcutChoices()
    let close: () -> Void

    init(close: @escaping () -> Void) {
        self.close = close
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Shortcuts on the Control wheel").font(.headline)
            Text("Run shortcuts from the Shortcuts app on files: each file goes in as the shortcut's input, and what it returns is saved beside the file. They appear after your loadouts.")
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Group {
                if model.loading {
                    ProgressView().controlSize(.small)
                } else if let problem = model.problem {
                    Text(problem).foregroundStyle(.red).fixedSize(horizontal: false, vertical: true)
                } else if model.available.isEmpty && model.missing.isEmpty {
                    Text("You don't have any shortcuts yet. Make one in the Shortcuts app that takes a file, then come back.")
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                } else {
                    List {
                        ForEach(model.available, id: \.self) { name in
                            Toggle(name, isOn: Binding(get: { model.isOn(name) },
                                                       set: { model.set(name, on: $0) }))
                        }
                        ForEach(model.missing, id: \.self) { name in
                            Toggle(isOn: Binding(get: { true }, set: { model.set(name, on: $0) })) {
                                Text(name) + Text("  no longer in Shortcuts").foregroundStyle(.secondary)
                            }
                        }
                    }
                    .frame(height: 220)
                }
            }
            Text("Only you choose what goes here. AI agents using Radlet can't run your shortcuts.")
                .font(.caption)
                .foregroundStyle(.secondary)
            HStack {
                Button("Open Shortcuts") { model.openShortcutsApp() }
                Spacer()
                Button("Done", action: close).keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 440)
        .onAppear { model.load() }
    }
}

/// Drafting a loadout from a sentence ("under 2 MB, as PDF") with Apple
/// Intelligence, on this Mac. The draft opens in the editor, unsaved, to
/// check first.
@MainActor
final class SentenceDraft: ObservableObject {
    @Published var text = ""
    @Published private(set) var working = false
    @Published private(set) var problem: String?

    static let examples = ["Under 2 MB, as PDF", "Small WebP for the web, no location", "Podcast-ready MP3"]

    var canDraft: Bool { !working && !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    func draft(open: @escaping (JSONValue) -> Void) {
        let request = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard canDraft else { return }
        working = true
        problem = nil
        Task {
            do {
                let result = try await EngineClient.shared.request("draft_recipe", ["text": .string(request)])
                working = false
                guard let recipe = result["recipe"] else {
                    problem = "Nothing came back. Try saying it another way."
                    return
                }
                Log.app("loadout drafted from a sentence")
                open(recipe)
            } catch let EngineError.remote(code, message) where code == "needs" {
                working = false
                problem = message
                InstallWindowController.shared.offer(.appleIntelligence, task: "Make a loadout from a sentence")
            } catch {
                working = false
                problem = error.localizedDescription
            }
        }
    }
}

struct SentenceDraftView: View {
    @StateObject private var model = SentenceDraft()
    let open: (JSONValue) -> Void
    let close: () -> Void

    init(open: @escaping (JSONValue) -> Void, close: @escaping () -> Void) {
        self.open = open
        self.close = close
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Label("Make a loadout from a sentence", systemImage: "sparkles").font(.headline)
            Text("Say what it should do. Apple Intelligence drafts the steps on this Mac, and you check them before saving.")
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            TextField("What should it do?", text: $model.text, axis: .vertical)
                .lineLimit(2...4)
                .textFieldStyle(.roundedBorder)
                .onSubmit { model.draft(open: open) }
            HStack(spacing: 6) {
                Text("Try:").font(.caption).foregroundStyle(.secondary)
                ForEach(SentenceDraft.examples, id: \.self) { example in
                    Button(example) { model.text = example }
                }
            }
            .controlSize(.small)
            if model.working {
                ProgressView("Drafting…").controlSize(.small)
            }
            if let problem = model.problem {
                Text(problem).foregroundStyle(.red).fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                Spacer()
                Button("Cancel", action: close).keyboardShortcut(.cancelAction)
                Button("Draft") { model.draft(open: open) }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!model.canDraft)
            }
        }
        .padding(20)
        .frame(width: 480)
    }
}
