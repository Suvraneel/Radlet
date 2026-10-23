import AppKit
import SwiftUI
import VeerKit

/// The Translate window: which language to translate documents and
/// subtitles into, and from. The work happens on this Mac (macOS 26), in
/// the Vision helper; languages are downloaded once in System Settings.
@MainActor
final class TranslateModel: ObservableObject {
    struct Language: Identifiable, Hashable {
        let id: String
        let name: String
    }

    private static let lastKey = "translateTo"

    @Published var to: String
    @Published var from = "auto"
    let languages: [Language]
    let files: [URL]
    private let finish: (JSONValue?) -> Void

    init(files: [URL], finish: @escaping (JSONValue?) -> Void) {
        self.files = files
        self.finish = finish
        let here = Locale.current
        let tags = ToolAction.translationLanguages
        languages = tags
            .map { Language(id: $0, name: here.localizedString(forIdentifier: $0) ?? $0) }
            .sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
        let last = UserDefaults.standard.string(forKey: Self.lastKey).flatMap { tags.contains($0) ? $0 : nil }
        to = last ?? ToolAction.defaultTranslationTarget(preferred: Locale.preferredLanguages, available: tags)
    }

    /// The system translation that files need arrived in macOS 26.
    static var supported: Bool {
        ProcessInfo.processInfo.isOperatingSystemAtLeast(OperatingSystemVersion(majorVersion: 26, minorVersion: 0,
                                                                                 patchVersion: 0))
    }

    var action: JSONValue { ToolAction.translate(to: to, from: from) }

    var canApply: Bool { Self.supported && from != to }

    var note: String {
        guard Self.supported else { return "Translating files needs macOS 26 or later." }
        if from == to { return "Choose two different languages." }
        if files.count == 1 {
            let f = files[0]
            let ext = f.pathExtension.isEmpty ? "" : "." + f.pathExtension
            return "Saved next to it as \u{201C}\(f.deletingPathExtension().lastPathComponent) (\(to))\(ext)\u{201D}. "
                + "Translated on this Mac."
        }
        return "Each file saved next to the original, with (\(to)) in its name. Translated on this Mac."
    }

    func apply() {
        UserDefaults.standard.set(to, forKey: Self.lastKey)
        finish(action)
    }

    func openSettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.Localization-Settings.extension") {
            NSWorkspace.shared.open(url)
        }
    }

    func cancel() { finish(nil) }

    func saveRecipe() {
        let name = languages.first { $0.id == to }?.name ?? to
        RecipeSaver.save(action, suggested: "Translate into \(name)")
    }
}

struct TranslateView: View {
    @ObservedObject var model: TranslateModel

    var body: some View {
        VStack(spacing: 0) {
            Form {
                Picker("Translate into", selection: $model.to) {
                    ForEach(model.languages) { l in Text(l.name).tag(l.id) }
                }
                Picker("From", selection: $model.from) {
                    Text("Detect automatically").tag("auto")
                    Divider()
                    ForEach(model.languages) { l in Text(l.name).tag(l.id) }
                }
                Section {
                    Text("Each language downloads once, in System Settings → General → Language & Region → "
                         + "Translation Languages. Code blocks, timings and layout are kept.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    Button("Open Language & Region Settings") { model.openSettings() }
                }
            }
            .formStyle(.grouped)
            Divider()
            ToolFooter(note: model.note, applyTitle: "Translate", canApply: model.canApply,
                       cancel: { model.cancel() }, apply: { model.apply() }, saveRecipe: { model.saveRecipe() })
        }
        .frame(width: 520)
    }
}
