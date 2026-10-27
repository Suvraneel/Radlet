import AppKit
import AVFoundation
import AVKit
import Combine
import PDFKit
import SwiftUI
import RadletKit

// Tool windows for PDFs (page numbers, watermark, password) and for audio
// and video (trim, make GIF). Same rules as ToolViews.swift: models are
// ObservableObjects, never `@State`.

// MARK: - Save as Recipe

@MainActor
enum RecipeSaver {
    /// Asks for a name, then saves `action`'s steps as a recipe on the
    /// Control wheel.
    static func save(_ action: JSONValue?, suggested: String) {
        guard let action, case .array(let steps)? = action["steps"], !steps.isEmpty else { return }
        let alert = NSAlert()
        alert.messageText = "Save as Loadout"
        alert.informativeText = "Loadouts appear on the Control wheel, ready for other files."
        let field = NSTextField(string: suggested)
        field.frame = NSRect(x: 0, y: 0, width: 280, height: 24)
        alert.accessoryView = field
        alert.addButton(withTitle: "Save")
        alert.addButton(withTitle: "Cancel")
        alert.window.initialFirstResponder = field
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let name = field.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return }
        Task {
            do {
                _ = try await EngineClient.shared.request("save_recipe", [
                    "recipe": .object(["name": .string(name), "steps": .array(steps)]),
                    "recipes_dir": .string(Prefs.loadoutsDir.path),
                ])
                Log.app("saved a recipe from a tool window")
                AppState.shared.notify("Saved loadout \u{201C}\(name)\u{201D}", detail: "Hold Control while dragging to use it")
            } catch {
                let failure = NSAlert()
                failure.messageText = "Couldn't save the loadout"
                failure.informativeText = error.localizedDescription
                failure.runModal()
            }
        }
    }
}

// MARK: - PDF preview

/// Runs a PDF tool on the first file into a scratch folder and shows one
/// page of the result.
@MainActor
final class PDFPreview: ObservableObject {
    @Published private(set) var image: NSImage?
    @Published private(set) var working = false
    @Published private(set) var message: String?
    /// Which page to show (from 0).
    var page = 0
    private let file: URL
    private let scratch = ScratchFolder()
    private var current: PreviewRun?
    private var pending: Task<Void, Never>?
    private var lastFolder: URL?

    init(file: URL) {
        self.file = file
        image = Self.render(file, page: 0)
    }

    static func render(_ url: URL, page: Int) -> NSImage? {
        guard let doc = PDFDocument(url: url), !doc.isLocked, doc.pageCount > 0,
              let p = doc.page(at: min(max(page, 0), doc.pageCount - 1)) else { return nil }
        return p.thumbnail(of: NSSize(width: 1000, height: 1300), for: .cropBox)
    }

    /// Shows `action` applied, after a short pause so typing stays smooth.
    func update(_ action: JSONValue?) {
        pending?.cancel()
        current?.cancel()
        current = nil
        guard let action else {
            working = false
            return
        }
        pending = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 350_000_000)
            guard !Task.isCancelled else { return }
            self?.run(action)
        }
    }

    private func run(_ action: JSONValue) {
        guard let folder = scratch.fresh() else { return }
        if let old = lastFolder { try? FileManager.default.removeItem(at: old) }
        lastFolder = folder
        let run = PreviewRun()
        current = run
        working = true
        message = nil
        let file = self.file
        Task { [weak self] in
            let outcome = try? await run.run(file: file, action: action, folder: folder)
            guard let self, run === self.current else { return }
            self.scratch.tidy()
            self.working = false
            if let o = outcome, o.status == "done", let made = o.createdItems.first {
                self.image = Self.render(URL(fileURLWithPath: made), page: self.page)
            } else if let o = outcome {
                self.message = o.error?.message
            }
        }
    }

    func stop() {
        pending?.cancel()
        current?.cancel()
        scratch.remove()
    }
}

/// The preview pane: a page with a soft shadow, and a spinner while working.
struct PDFPreviewPane: View {
    @ObservedObject var preview: PDFPreview

    var body: some View {
        ZStack(alignment: .topTrailing) {
            Color(nsColor: .underPageBackgroundColor)
            if let image = preview.image {
                Image(nsImage: image)
                    .resizable()
                    .aspectRatio(contentMode: .fit)
                    .shadow(color: .black.opacity(0.2), radius: 4, y: 1)
                    .padding(20)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                Text("No preview for this PDF").foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            if preview.working {
                ProgressView().controlSize(.small).padding(12)
            }
        }
    }
}

// MARK: - Page numbers

@MainActor
final class PageNumbersModel: ObservableObject {
    @Published var position: ToolAction.NumberPosition = .bottomCenter { didSet { refresh() } }
    @Published var style: ToolAction.NumberStyle = .number { didSet { refresh() } }
    @Published var startText = "1" { didSet { refresh() } }
    @Published var size: Double = 10 { didSet { refresh() } }
    @Published var skipFirst = false { didSet { refresh() } }
    let files: [URL]
    let preview: PDFPreview
    private let finish: (JSONValue?) -> Void
    private var previewChanges: AnyCancellable?

    init(files: [URL], finish: @escaping (JSONValue?) -> Void) {
        self.files = files
        self.finish = finish
        preview = PDFPreview(file: files[0])
        // The footer shows the preview's errors, so refresh with it.
        previewChanges = preview.objectWillChange.sink { [weak self] _ in self?.objectWillChange.send() }
        refresh()
    }

    var start: Int? {
        guard let n = Int(startText.trimmingCharacters(in: .whitespaces)), n >= 0, n <= 999_999 else { return nil }
        return n
    }

    var action: JSONValue? {
        guard let start else { return nil }
        return ToolAction.pageNumbers(position: position, style: style, start: start,
                                      size: (size * 2).rounded() / 2, skipFirst: skipFirst)
    }

    private func refresh() {
        preview.page = skipFirst ? 1 : 0
        preview.update(action)
    }

    var note: String {
        if start == nil { return "Enter the first page's number." }
        if let m = preview.message { return m }
        return files.count > 1 ? "Numbers every page of all \(files.count) PDFs." : "Each page gets its number; the original stays as it is."
    }

    func cancel() { finish(nil) }
    func apply() { if let action { finish(action) } }
    func saveRecipe() { RecipeSaver.save(action, suggested: "Number pages") }
    func stop() { preview.stop() }
}

struct PageNumbersView: View {
    @ObservedObject var model: PageNumbersModel

    private let grid: [[ToolAction.NumberPosition]] = [
        [.topLeft, .topCenter, .topRight],
        [.bottomLeft, .bottomCenter, .bottomRight],
    ]

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 0) {
                Form {
                    Picker("Style", selection: $model.style) {
                        ForEach(ToolAction.NumberStyle.allCases) { s in Text(s.example).tag(s) }
                    }
                    LabeledContent("Position") {
                        VStack(spacing: 4) {
                            ForEach(0..<2, id: \.self) { row in
                                HStack(spacing: 4) {
                                    ForEach(grid[row]) { p in
                                        Button { model.position = p } label: {
                                            RoundedRectangle(cornerRadius: 3)
                                                .fill(model.position == p ? Color.accentColor : Color.primary.opacity(0.12))
                                                .frame(width: 26, height: 14)
                                        }
                                        .buttonStyle(.plain)
                                        .help(p.rawValue.replacingOccurrences(of: "_", with: " ").capitalized)
                                    }
                                }
                                if row == 0 { Spacer().frame(height: 18) }
                            }
                        }
                        .padding(6)
                        .background(RoundedRectangle(cornerRadius: 5).stroke(Color.primary.opacity(0.2)))
                    }
                    TextField("Start at", text: $model.startText)
                    LabeledContent("Size") {
                        HStack {
                            Slider(value: $model.size, in: 6...24)
                            Text("\(Int(model.size)) pt").monospacedDigit().frame(width: 40, alignment: .trailing)
                        }
                    }
                    Toggle("Leave the first page (cover) unnumbered", isOn: $model.skipFirst)
                }
                .formStyle(.grouped)
                .frame(width: 330)
                Divider()
                PDFPreviewPane(preview: model.preview)
            }
            Divider()
            ToolFooter(note: model.note, applyTitle: "Add Numbers", canApply: model.action != nil,
                       cancel: { model.cancel() }, apply: { model.apply() },
                       saveRecipe: { model.saveRecipe() })
        }
        .frame(minWidth: 760, idealWidth: 860, maxWidth: .infinity, minHeight: 460, idealHeight: 560, maxHeight: .infinity)
    }
}

// MARK: - Watermark

@MainActor
final class WatermarkModel: ObservableObject {
    @Published var text = "DRAFT" { didSet { refresh() } }
    @Published var opacity: Double = 0.15 { didSet { refresh() } }
    @Published var diagonal = true { didSet { refresh() } }
    let files: [URL]
    let preview: PDFPreview
    private let finish: (JSONValue?) -> Void
    private var previewChanges: AnyCancellable?

    init(files: [URL], finish: @escaping (JSONValue?) -> Void) {
        self.files = files
        self.finish = finish
        preview = PDFPreview(file: files[0])
        // The footer shows the preview's errors, so refresh with it.
        previewChanges = preview.objectWillChange.sink { [weak self] _ in self?.objectWillChange.send() }
        refresh()
    }

    var action: JSONValue? {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty else { return nil }
        return ToolAction.watermark(text: t, opacity: opacity, diagonal: diagonal)
    }

    private func refresh() { preview.update(action) }

    var note: String {
        if action == nil { return "Type the text to print on every page." }
        if let m = preview.message { return m.prefix(1).uppercased() + String(m.dropFirst()) }
        return files.count > 1 ? "Marks every page of all \(files.count) PDFs." : "The original stays as it is."
    }

    func cancel() { finish(nil) }
    func apply() { if let action { finish(action) } }
    func saveRecipe() { RecipeSaver.save(action, suggested: "Watermark \(text)") }
    func stop() { preview.stop() }
}

struct WatermarkView: View {
    @ObservedObject var model: WatermarkModel

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 0) {
                Form {
                    TextField("Text", text: $model.text)
                    LabeledContent("Strength") {
                        HStack {
                            Slider(value: $model.opacity, in: 0.05...0.6)
                            Text("\(Int((model.opacity * 100).rounded()))%").monospacedDigit()
                                .frame(width: 40, alignment: .trailing)
                        }
                    }
                    Picker("Direction", selection: $model.diagonal) {
                        Text("Corner to corner").tag(true)
                        Text("Across the middle").tag(false)
                    }
                    .pickerStyle(.radioGroup)
                    Text("Latin letters, numbers and punctuation.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                .formStyle(.grouped)
                .frame(width: 330)
                Divider()
                PDFPreviewPane(preview: model.preview)
            }
            Divider()
            ToolFooter(note: model.note, applyTitle: "Add Watermark", canApply: model.action != nil,
                       cancel: { model.cancel() }, apply: { model.apply() },
                       saveRecipe: { model.saveRecipe() })
        }
        .frame(minWidth: 760, idealWidth: 860, maxWidth: .infinity, minHeight: 460, idealHeight: 560, maxHeight: .infinity)
    }
}

// MARK: - Password

/// Adds or removes a PDF's password, locks a new ZIP, or opens a locked
/// one. The password goes to the engine for this one job and is never
/// saved.
@MainActor
final class PasswordModel: ObservableObject {
    enum Mode: String, CaseIterable, Identifiable {
        case protect, unlock
        var id: String { rawValue }
    }

    enum Purpose {
        case pdf, zip, unzip
    }

    @Published var mode: Mode
    @Published var password = ""
    @Published var confirm = ""
    let files: [URL]
    let purpose: Purpose
    /// Some PDFs are locked and some aren't, so let the user choose.
    let mixed: Bool
    private let finish: (JSONValue?) -> Void

    init(files: [URL], purpose: Purpose = .pdf, finish: @escaping (JSONValue?) -> Void) {
        self.files = files
        self.purpose = purpose
        self.finish = finish
        switch purpose {
        case .pdf:
            let locked = files.map { PDFDocument(url: $0)?.isLocked == true }
            mode = locked.allSatisfy { $0 } ? .unlock : .protect
            mixed = locked.contains(true) && locked.contains(false)
        case .zip:
            mode = .protect
            mixed = false
        case .unzip:
            mode = .unlock
            mixed = false
        }
    }

    var problem: String? {
        switch mode {
        case .protect:
            if password.isEmpty { return nil }
            if confirm.isEmpty { return "Type the password again to confirm it." }
            return password == confirm ? nil : "The passwords don't match."
        case .unlock:
            return nil
        }
    }

    var canApply: Bool {
        !password.isEmpty && problem == nil && (mode == .unlock || password == confirm)
    }

    var title: String {
        switch (purpose, mode) {
        case (.zip, _): return "Zip with a password"
        case (.unzip, _): return "Unzip with the password"
        case (.pdf, .protect): return "Lock with a password"
        case (.pdf, .unlock): return "Remove the password"
        }
    }

    var note: String {
        if let problem { return problem }
        switch (purpose, mode) {
        case (.zip, _):
            return "Saves one ZIP with every file locked (AES-256); names stay visible. "
                + "Keep the password safe: there's no way to recover it."
        case (.unzip, _):
            return "Extracts the files next to the ZIP."
        case (.pdf, .protect):
            return "Saves a locked copy (AES-256). Keep the password safe: there's no way to recover it."
        case (.pdf, .unlock):
            return "Saves a copy that opens without a password."
        }
    }

    var applyTitle: String {
        switch purpose {
        case .zip: return "Zip"
        case .unzip: return "Unzip"
        case .pdf: return mode == .protect ? "Lock" : "Unlock"
        }
    }

    func cancel() { finish(nil) }
    func apply() {
        guard canApply else { return }
        switch (purpose, mode) {
        case (.zip, _): finish(ToolAction.zipLocked(password: password))
        case (.unzip, _): finish(ToolAction.unzipLocked(password: password))
        case (.pdf, .protect): finish(ToolAction.protectPDF(password: password))
        case (.pdf, .unlock): finish(ToolAction.unlockPDF(password: password))
        }
    }
}

struct PasswordView: View {
    @ObservedObject var model: PasswordModel

    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 14) {
                if model.mixed {
                    Picker("", selection: $model.mode) {
                        Text("Add a password").tag(PasswordModel.Mode.protect)
                        Text("Remove the password").tag(PasswordModel.Mode.unlock)
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                } else {
                    Label(model.title, systemImage: model.mode == .protect ? "lock" : "lock.open")
                        .font(.headline)
                }
                SecureField(model.mode == .protect ? "New password" : "Password", text: $model.password)
                    .textFieldStyle(.roundedBorder)
                if model.mode == .protect {
                    SecureField("Confirm password", text: $model.confirm)
                        .textFieldStyle(.roundedBorder)
                }
            }
            .padding(20)
            Divider()
            ToolFooter(note: model.note, applyTitle: model.applyTitle,
                       canApply: model.canApply, cancel: { model.cancel() }, apply: { model.apply() })
        }
        .frame(width: 460)
    }
}

// MARK: - Black out text

/// Words, phrases and kinds of details to remove from PDFs for real.
@MainActor
final class RedactTextModel: ObservableObject {
    @Published var words = ""
    @Published var emails = false
    @Published var phones = false
    @Published var cards = false
    let files: [URL]
    private let finish: (JSONValue?) -> Void

    init(files: [URL], finish: @escaping (JSONValue?) -> Void) {
        self.files = files
        self.finish = finish
    }

    var phrases: [String] {
        words.components(separatedBy: .newlines)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
    }

    var canApply: Bool { !phrases.isEmpty || emails || phones || cards }

    var note: String {
        guard canApply else { return "Type what to black out, one word or phrase a line, or tick a kind of detail." }
        return "Pages with a match become pictures with black boxes and fresh searchable text, so nothing under "
            + "a box can be copied. Other pages stay as they are."
    }

    var action: JSONValue {
        ToolAction.redactText(words: phrases, emails: emails, phones: phones, cards: cards)
    }

    func cancel() { finish(nil) }
    func apply() {
        guard canApply else { return }
        finish(action)
    }
}

struct RedactTextView: View {
    @ObservedObject var model: RedactTextModel

    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 12) {
                Label("Black out text", systemImage: "eye.slash")
                    .font(.headline)
                Text("Words and phrases, one a line (any case):")
                    .foregroundStyle(.secondary)
                TextEditor(text: $model.words)
                    .font(.body)
                    .frame(minHeight: 90)
                    .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.secondary.opacity(0.3)))
                Toggle("Every email address", isOn: $model.emails)
                Toggle("Every phone number", isOn: $model.phones)
                Toggle("Every card number", isOn: $model.cards)
            }
            .padding(20)
            Divider()
            ToolFooter(note: model.note, applyTitle: "Black Out", canApply: model.canApply,
                       cancel: { model.cancel() }, apply: { model.apply() })
        }
        .frame(width: 480)
    }
}

// MARK: - Trim and Make GIF

@MainActor
final class TrimModel: ObservableObject {
    /// nil while checking whether the player can trim this file.
    @Published private(set) var playerTrims: Bool?
    @Published var startText = "0:00.0"
    @Published var endText = ""
    let files: [URL]
    let gif: Bool
    let player: AVPlayer
    private let finish: (JSONValue?) -> Void
    private var waiting: Task<Void, Never>?

    init(files: [URL], gif: Bool, finish: @escaping (JSONValue?) -> Void) {
        self.files = files
        self.gif = gif
        self.finish = finish
        player = AVPlayer(url: files[0])
    }

    /// Starts QuickTime-style trimming once the player has the file ready;
    /// falls back to typed times for formats it can't play.
    func attach(_ view: AVPlayerView) {
        waiting?.cancel()
        waiting = Task { [weak self, weak view] in
            for _ in 0..<50 {
                try? await Task.sleep(nanoseconds: 200_000_000)
                guard let self, let view, !Task.isCancelled else { return }
                if view.window != nil && view.canBeginTrimming {
                    self.startTrimming(view)
                    return
                }
            }
            self?.playerTrims = false
        }
    }

    /// Uses the completion handler rather than `await`: an awaiting task
    /// would keep this window's player alive until the user chose Trim or
    /// Cancel, even after the window closed.
    private func startTrimming(_ view: AVPlayerView) {
        playerTrims = true
        view.beginTrimming { [weak self] result in
            Task { @MainActor in self?.trimmed(result) }
        }
    }

    private func trimmed(_ result: AVPlayerViewTrimResult) {
        guard result == .okButton, let item = player.currentItem else {
            finish(nil)
            return
        }
        let start = item.reversePlaybackEndTime
        let end = item.forwardPlaybackEndTime
        let s = start.isValid && start.isNumeric ? max(CMTimeGetSeconds(start), 0) : 0
        var e: Double? = end.isValid && end.isNumeric ? CMTimeGetSeconds(end) : nil
        if let d = item.duration.isNumeric ? CMTimeGetSeconds(item.duration) : nil, let ee = e, ee >= d - 0.05 {
            e = nil  // up to the end
        }
        submit(start: s, end: e)
    }

    private func submit(start: Double, end: Double?) {
        if !gif && start <= 0.05 && end == nil {
            finish(nil)  // the whole file: nothing to trim
            return
        }
        finish(gif ? ToolAction.gif(start: start, end: end) : ToolAction.trim(start: start, end: end))
    }

    var typedStart: Double? { TimeText.parse(startText) }
    var typedEnd: Double? {
        let t = endText.trimmingCharacters(in: .whitespaces)
        return t.isEmpty ? nil : TimeText.parse(t)
    }

    var typedProblem: String? {
        guard let s = typedStart else { return "Enter the start as seconds or minutes:seconds." }
        let endEmpty = endText.trimmingCharacters(in: .whitespaces).isEmpty
        if !endEmpty {
            guard let e = typedEnd else { return "Enter the end as seconds or minutes:seconds." }
            if e <= s { return "The end must come after the start." }
        } else if s <= 0 && !gif {
            return "Enter where to start or end."
        }
        return nil
    }

    var note: String {
        if playerTrims == true {
            return files.count > 1 ? "Drag the yellow handles, then click Trim. The same part is kept from all \(files.count) files."
                : "Drag the yellow handles, then click Trim."
        }
        if playerTrims == false { return typedProblem ?? "Leave the end empty to keep everything after the start." }
        return "Opening…"
    }

    func applyTyped() {
        guard typedProblem == nil, let s = typedStart else { return }
        submit(start: s, end: typedEnd)
    }

    func cancel() { finish(nil) }
    func stop() {
        waiting?.cancel()
        player.pause()
    }
}

struct TrimPlayer: NSViewRepresentable {
    let model: TrimModel

    func makeNSView(context: Context) -> AVPlayerView {
        let view = AVPlayerView()
        view.controlsStyle = .inline
        view.showsFullScreenToggleButton = false
        view.player = model.player
        model.attach(view)
        return view
    }

    func updateNSView(_ nsView: AVPlayerView, context: Context) {}
}

struct TrimView: View {
    @ObservedObject var model: TrimModel

    var body: some View {
        VStack(spacing: 0) {
            if model.playerTrims == false {
                Form {
                    TextField("Start", text: $model.startText)
                    TextField("End (empty: to the end)", text: $model.endText)
                    Text("This format can't be previewed here, so type the times, such as 1:30 or 90.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                .formStyle(.grouped)
                .frame(minHeight: 160)
                Divider()
                ToolFooter(note: model.note, applyTitle: model.gif ? "Make GIF" : "Trim",
                           canApply: model.typedProblem == nil,
                           cancel: { model.cancel() }, apply: { model.applyTyped() })
            } else {
                TrimPlayer(model: model)
                Divider()
                HStack {
                    Text(model.note).font(.callout).foregroundStyle(.secondary)
                    Spacer()
                    Button("Cancel") { model.cancel() }
                        .keyboardShortcut(.cancelAction)
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 10)
            }
        }
        .frame(minWidth: 560, idealWidth: 760, maxWidth: .infinity, minHeight: 300, idealHeight: 520, maxHeight: .infinity)
    }
}
