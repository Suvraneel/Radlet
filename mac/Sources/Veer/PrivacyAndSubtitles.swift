import AppKit
import Speech
import SwiftUI
import VeerKit

// Windows for the Privacy check and Make subtitles tools. State lives in
// ObservableObjects, never `@State` (see Settings.swift).

// MARK: - Privacy check

/// What the selected files reveal, from the engine's `privacy` method,
/// with Remove Metadata as the way out.
@MainActor
final class PrivacyModel: ObservableObject {
    @Published private(set) var reports: [PrivacyReport] = []
    @Published private(set) var loading = true
    @Published private(set) var failure: String?
    let files: [URL]
    private let finish: (JSONValue?) -> Void

    init(files: [URL], finish: @escaping (JSONValue?) -> Void) {
        self.files = files
        self.finish = finish
        Task { [weak self] in await self?.load() }
    }

    private func load() async {
        do {
            let result = try await EngineClient.shared.request(
                "privacy", ["paths": .array(files.map { .string($0.path) })])
            reports = try PrivacyReport.list(from: result)
            let found = reports.reduce(0) { $0 + $1.findings.count }
            Log.app("privacy check: \(found) findings in \(reports.count) files")
        } catch {
            failure = error.localizedDescription
        }
        loading = false
    }

    var headline: String {
        if loading { return "Checking what \(files.count == 1 ? "this file reveals" : "these files reveal")…" }
        return failure ?? PrivacySummary.headline(reports)
    }

    var canClean: Bool { !loading && !PrivacySummary.filesToClean(reports).isEmpty }

    var note: String {
        if loading { return "Everything is read on this Mac." }
        if failure != nil { return "Veer couldn't read these files." }
        let stays = PrivacySummary.staying(reports)
        let staying = stays.isEmpty ? "" : " These stay: \(stays.joined(separator: ", ").lowercased())."
        if !canClean {
            return stays.isEmpty ? "There's nothing to remove." : "Remove Metadata can't take these out.\(staying)"
        }
        return "Saves cleaned copies; the originals stay as they are.\(staying)"
    }

    func cancel() { finish(nil) }

    func apply() {
        finish(.object(["kind": .string("steps"), "steps": .array([.object(["op": .string("strip_metadata")])])]))
    }
}

struct PrivacyView: View {
    @ObservedObject var model: PrivacyModel

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: "hand.raised").font(.title2).foregroundStyle(.secondary)
                Text(model.headline).font(.headline).lineLimit(2)
                Spacer()
                if model.loading { ProgressView().controlSize(.small) }
            }
            .padding(14)
            Divider()
            List {
                ForEach(model.reports) { report in
                    Section(report.name) {
                        if let problem = report.error {
                            Label(problem.message, systemImage: "exclamationmark.triangle")
                                .foregroundStyle(.secondary)
                        } else if report.findings.isEmpty {
                            Label("Nothing revealing found", systemImage: "checkmark.seal")
                                .foregroundStyle(.secondary)
                        }
                        ForEach(report.findings) { finding in
                            FindingRow(finding: finding)
                        }
                    }
                }
            }
            Divider()
            ToolFooter(note: model.note, applyTitle: "Remove Metadata", canApply: model.canClean,
                       cancel: { model.cancel() }, apply: { model.apply() })
        }
        .frame(minWidth: 540, idealWidth: 620, maxWidth: .infinity, minHeight: 380, idealHeight: 520, maxHeight: .infinity)
    }
}

private struct FindingRow: View {
    let finding: PrivacyFinding

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Image(systemName: finding.symbol)
                .foregroundStyle(finding.kind == "location" ? Color.red : Color.secondary)
                .frame(width: 18)
            VStack(alignment: .leading, spacing: 2) {
                Text(finding.label).font(.callout.weight(.medium))
                Text(finding.detail)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .lineLimit(3)
                    .textSelection(.enabled)
            }
            Spacer(minLength: 8)
            if let url = finding.mapsURL {
                Button("Show in Maps") { NSWorkspace.shared.open(url) }
                    .controlSize(.small)
            }
            if !finding.removable {
                Text("Stays")
                    .font(.caption)
                    .foregroundStyle(.orange)
                    .help("Remove Metadata leaves this. Edit the file to take it out.")
            }
        }
        .padding(.vertical, 2)
    }
}

// MARK: - Make subtitles

/// Format and language for subtitles. Asks for speech recognition
/// permission here, in the app, so the helper that does the work never
/// has to.
@MainActor
final class SubtitlesModel: ObservableObject {
    struct Language: Identifiable, Hashable {
        let id: String
        let name: String
    }

    @Published var format = "srt"
    @Published var language = "auto"
    @Published private(set) var message: String?
    @Published private(set) var asking = false
    /// The window closed: a late permission answer mustn't start a job.
    private var stopped = false
    let languages: [Language]
    let files: [URL]
    private let finish: (JSONValue?) -> Void

    init(files: [URL], finish: @escaping (JSONValue?) -> Void) {
        self.files = files
        self.finish = finish
        let here = Locale.current
        let named = SFSpeechRecognizer.supportedLocales()
            .map { Language(id: $0.identifier, name: here.localizedString(forIdentifier: $0.identifier) ?? $0.identifier) }
            .sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
        let mine = here.localizedString(forIdentifier: here.identifier) ?? here.identifier
        languages = [Language(id: "auto", name: "Same as this Mac (\(mine))")] + named
    }

    var action: JSONValue {
        ToolAction.transcribe(to: format, language: language == "auto" ? nil : language)
    }

    var note: String {
        if let message { return message }
        let what = format == "txt" ? "A transcript" : "Subtitles"
        return "\(what) saved next to \(files.count == 1 ? "the file" : "each file"). Speech is recognised on this Mac."
    }

    var denied: Bool { message != nil }

    func apply() {
        switch SFSpeechRecognizer.authorizationStatus() {
        case .authorized:
            finish(action)
        case .notDetermined:
            asking = true
            SFSpeechRecognizer.requestAuthorization { status in
                DispatchQueue.main.async {
                    MainActor.assumeIsolated { self.answered(status) }
                }
            }
        default:
            refuse()
        }
    }

    private func answered(_ status: SFSpeechRecognizerAuthorizationStatus) {
        asking = false
        guard !stopped else { return }
        if status == .authorized {
            finish(action)
        } else {
            refuse()
        }
    }

    private func refuse() {
        message = "Speech recognition is off for Veer. Turn it on in System Settings, then try again."
        Log.app("subtitles: speech recognition not allowed")
    }

    func openSettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_SpeechRecognition") {
            NSWorkspace.shared.open(url)
        }
    }

    func cancel() { finish(nil) }
    func stop() { stopped = true }
    func saveRecipe() { RecipeSaver.save(action, suggested: format == "txt" ? "Transcript" : "Subtitles") }
}

struct SubtitlesView: View {
    @ObservedObject var model: SubtitlesModel

    var body: some View {
        VStack(spacing: 0) {
            Form {
                Picker("Save as", selection: $model.format) {
                    Text("Subtitles (SRT)").tag("srt")
                    Text("Subtitles (WebVTT)").tag("vtt")
                    Text("Transcript (text)").tag("txt")
                }
                Picker("Language", selection: $model.language) {
                    ForEach(model.languages) { l in Text(l.name).tag(l.id) }
                }
                if model.denied {
                    Button("Open Privacy & Security Settings") { model.openSettings() }
                }
            }
            .formStyle(.grouped)
            Divider()
            ToolFooter(note: model.note, applyTitle: model.asking ? "Waiting…" : "Make Subtitles",
                       canApply: !model.asking, cancel: { model.cancel() }, apply: { model.apply() },
                       saveRecipe: { model.saveRecipe() })
        }
        .frame(width: 520)
    }
}

// MARK: - AI agents

/// How to add Veer to an MCP client (Claude Desktop, Claude Code…).
enum MCPSetup {
    static var helper: String {
        Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/veer").path
    }

    static var desktopConfig: String {
        let escaped = helper.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"")
        return """
        {
          "mcpServers": {
            "veer": { "command": "\(escaped)", "args": ["mcp"] }
          }
        }
        """
    }

    static var claudeCode: String {
        let quoted = "'" + helper.replacingOccurrences(of: "'", with: "'\\''") + "'"
        return "claude mcp add veer -- \(quoted) mcp"
    }

    static func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }
}
