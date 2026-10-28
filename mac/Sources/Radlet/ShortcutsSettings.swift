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
