import AppKit
import SwiftUI
import RadletKit

// Settings → Recipes: list, edit, duplicate and delete recipes. Recipes
// are .toml files in the recipes folder; the engine reads and writes them
// (`recipes`, `save_recipe`) so the app never parses TOML itself.

/// One step in the editor.
@MainActor
final class StepModel: ObservableObject, Identifiable {
    let id = UUID()
    @Published var fields: StepFields {
        didSet { if fields != oldValue { onChange() } }
    }
    var onChange: () -> Void = {}

    init(_ step: JSONValue) {
        fields = StepFields(step)
    }

    var title: String {
        switch fields.build() {
        case .success(let v): return StepText.describe(v)
        case .failure: return StepText.describe(.object(fields.original))
        }
    }

    var problem: String? {
        if case .failure(let p) = fields.build() { return p.message }
        return nil
    }
}

/// The recipe being edited (or viewed, for a built-in one).
@MainActor
final class RecipeDraft: ObservableObject {
    /// The file it was loaded from; nil for a new recipe or a built-in one.
    let path: URL?
    let builtin: Bool
    @Published var name: String {
        didSet { if name != oldValue { touch() } }
    }
    @Published var summary: String {
        didSet { if summary != oldValue { touch() } }
    }
    @Published private(set) var steps: [StepModel] = []
    @Published private(set) var dirty: Bool
    @Published var saving = false
    @Published var error: String?

    init(name: String, summary: String, steps: [JSONValue], path: URL?, builtin: Bool, dirty: Bool) {
        self.name = name
        self.summary = summary
        self.path = path
        self.builtin = builtin
        self.dirty = dirty
        self.steps = steps.map { StepModel($0) }
        for s in self.steps { s.onChange = { [weak self] in self?.touch() } }
    }

    var built: Result<JSONValue, EditProblem> {
        RecipeCheck.build(name: name, description: summary, steps: steps.map(\.fields))
    }

    var problem: String? {
        if case .failure(let p) = built { return p.message }
        return nil
    }

    private func touch() {
        dirty = true
        error = nil
    }

    func markSaved() {
        dirty = false
    }

    func add(_ step: JSONValue) {
        let s = StepModel(step)
        s.onChange = { [weak self] in self?.touch() }
        steps.append(s)
        touch()
    }

    func remove(_ step: StepModel) {
        steps.removeAll { $0.id == step.id }
        touch()
    }

    func move(_ step: StepModel, by offset: Int) {
        guard let i = steps.firstIndex(where: { $0.id == step.id }) else { return }
        let j = i + offset
        guard steps.indices.contains(j) else { return }
        steps.swapAt(i, j)
        touch()
    }
}

/// Every recipe the wheel offers: built-ins, then the user's files.
@MainActor
final class RecipeLibrary: ObservableObject {
    typealias Item = RecipeEntry

    @Published private(set) var items: [Item] = []
    /// Recipe files that couldn't be read, with why.
    @Published private(set) var problems: [String] = []
    @Published private(set) var draft: RecipeDraft?
    @Published var selection: String? {
        didSet {
            guard selection != oldValue, !quiet else { return }
            guard let draft, draft.dirty else {
                openSelected()
                return
            }
            // Ask after the list has finished updating its selection.
            let previous = oldValue
            DispatchQueue.main.async { [weak self] in
                MainActor.assumeIsolated {
                    guard let self else { return }
                    if self.confirmDiscard() {
                        self.openSelected()
                    } else {
                        self.select(previous, open: false)
                    }
                }
            }
        }
    }
    /// Set while the selection changes in code rather than by a click.
    private var quiet = false

    var user: [Item] { items.filter { !$0.builtin } }
    var builtins: [Item] { items.filter(\.builtin) }
    var selectedItem: Item? { items.first { $0.id == selection } }

    func refresh(select wanted: String? = nil) {
        Task {
            do {
                let result = try await EngineClient.shared.request(
                    "recipes", ["recipes_dir": .string(Prefs.loadoutsDir.path)])
                apply(result, select: wanted)
            } catch {
                problems = [error.localizedDescription]
            }
        }
    }

    private func apply(_ result: JSONValue, select wanted: String?) {
        let listing = RecipeListing(result)
        let list = listing.recipes
        items = list
        problems = listing.problems

        let target = wanted ?? selection
        if let target, list.contains(where: { $0.id == target }) {
            // Reload unless there are unsaved edits to keep.
            if target != selection || !(draft?.dirty ?? false) { select(target, open: true) }
        } else if let draft, draft.dirty, draft.path == nil, !draft.builtin {
            // A new recipe not saved yet: keep editing it.
        } else {
            select(list.first { !$0.builtin }?.id ?? list.first?.id, open: true)
        }
    }

    private func select(_ id: String?, open: Bool) {
        quiet = true
        selection = id
        quiet = false
        if open { openSelected() }
    }

    private func openSelected() {
        guard let item = selectedItem else {
            draft = nil
            return
        }
        draft = RecipeDraft(name: item.name, summary: item.description, steps: item.steps,
                            path: item.path, builtin: item.builtin, dirty: false)
    }

    /// True when there's nothing unsaved, or the user agrees to lose it.
    private func confirmDiscard() -> Bool {
        guard let draft, draft.dirty else { return true }
        let alert = NSAlert()
        let name = draft.name.trimmingCharacters(in: .whitespaces)
        alert.messageText = "Discard your changes to \u{201C}\(name.isEmpty ? "this loadout" : name)\u{201D}?"
        alert.addButton(withTitle: "Discard Changes")
        alert.addButton(withTitle: "Keep Editing")
        return alert.runModal() == .alertFirstButtonReturn
    }

    func newRecipe() {
        guard confirmDiscard() else { return }
        select(nil, open: false)
        draft = RecipeDraft(name: "New Loadout", summary: "",
                            steps: [.object(["op": .string("convert"), "to": .string("jpeg")])],
                            path: nil, builtin: false, dirty: true)
    }

    /// Copies the recipe shown (built-in or not) into a new, editable one.
    func duplicate() {
        guard let current = draft else { return }
        let steps = current.steps.map { s -> JSONValue in
            (try? s.fields.build().get()) ?? .object(s.fields.original)
        }
        let name = current.name.trimmingCharacters(in: .whitespaces)
        if current.dirty && !current.builtin && current.path != nil {
            guard confirmDiscard() else { return }
        }
        select(nil, open: false)
        draft = RecipeDraft(name: "\(name) copy", summary: current.summary, steps: steps,
                            path: nil, builtin: false, dirty: true)
    }

    func revert() {
        if selectedItem != nil {
            openSelected()
        } else {
            draft = nil
            select(user.first?.id ?? items.first?.id, open: true)
        }
    }

    func save() {
        guard let draft, !draft.saving, !draft.builtin else { return }
        let recipe: JSONValue
        switch draft.built {
        case .failure(let p):
            draft.error = p.message
            return
        case .success(let r):
            recipe = r
        }
        var params: [String: JSONValue] = ["recipe": recipe, "recipes_dir": .string(Prefs.loadoutsDir.path)]
        if let path = draft.path { params["replace"] = .string(path.path) }
        let request = params
        draft.saving = true
        Task {
            do {
                let result = try await EngineClient.shared.request("save_recipe", request)
                draft.saving = false
                draft.markSaved()
                Log.app("recipe saved from Settings")
                refresh(select: result["path"]?.string.map { RecipeEntry.key(URL(fileURLWithPath: $0)) })
            } catch {
                draft.saving = false
                draft.error = error.localizedDescription
            }
        }
    }

    /// Shares the loadout shown, as it is in the editor, as a link or a
    /// file anyone with Radlet can add.
    func share(asFile: Bool) {
        guard let draft else { return }
        let recipe: JSONValue
        switch draft.built {
        case .failure(let p):
            draft.error = p.message
            return
        case .success(let r):
            recipe = r
        }
        let name = draft.name.trimmingCharacters(in: .whitespaces)
        Task {
            do {
                let result = try await EngineClient.shared.request("recipe_toml", ["recipe": recipe])
                guard let text = result["text"]?.string, let item = SharedItem(kind: .loadout, text: text) else {
                    draft.error = "This loadout couldn't be shared."
                    return
                }
                if asFile {
                    ShareExport.saveFile(item, name: name)
                } else {
                    ShareExport.copyLink(item, name: name)
                }
            } catch {
                draft.error = error.localizedDescription
            }
        }
    }

    var canDelete: Bool { selectedItem?.path != nil }

    func deleteSelected() {
        guard let item = selectedItem, let path = item.path else { return }
        let alert = NSAlert()
        alert.messageText = "Move \u{201C}\(item.name)\u{201D} to the Trash?"
        alert.informativeText = "It leaves the Control wheel. You can put it back from the Trash."
        alert.addButton(withTitle: "Move to Trash")
        alert.addButton(withTitle: "Cancel")
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        NSWorkspace.shared.recycle([path]) { [weak self] _, error in
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard let self else { return }
                    if let error { Log.app("recipe delete: \(Log.code(error))") }
                    self.draft = nil
                    self.select(nil, open: false)
                    self.refresh()
                }
            }
        }
    }

    func revealFolder() {
        let dir = Prefs.loadoutsDir
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        if let path = selectedItem?.path {
            NSWorkspace.shared.activateFileViewerSelecting([path])
        } else {
            NSWorkspace.shared.open(dir)
        }
    }
}

struct RecipesSettings: View {
    @StateObject private var library = RecipeLibrary()

    var body: some View {
        HStack(spacing: 0) {
            VStack(spacing: 0) {
                List(selection: $library.selection) {
                    if !library.user.isEmpty {
                        Section("Yours") {
                            ForEach(library.user) { item in Text(item.name).lineLimit(1) }
                        }
                    }
                    Section("Built in") {
                        ForEach(library.builtins) { item in Text(item.name).lineLimit(1) }
                    }
                }
                if !library.problems.isEmpty {
                    VStack(alignment: .leading, spacing: 2) {
                        ForEach(library.problems, id: \.self) { p in
                            Text(p).font(.caption).foregroundStyle(.red).lineLimit(3)
                        }
                    }
                    .padding(6)
                }
                Divider()
                HStack(spacing: 2) {
                    Button(action: library.newRecipe) { Image(systemName: "plus") }
                        .help("New loadout")
                    Button(action: library.deleteSelected) { Image(systemName: "minus") }
                        .disabled(!library.canDelete)
                        .help("Move the loadout to the Trash")
                    Spacer()
                    Button(action: library.duplicate) { Image(systemName: "plus.square.on.square") }
                        .disabled(library.draft == nil)
                        .help("Duplicate")
                    Menu {
                        Button("Copy Link") { library.share(asFile: false) }
                        Button("Save as File…") { library.share(asFile: true) }
                    } label: {
                        Image(systemName: "square.and.arrow.up")
                    }
                    .menuStyle(.borderlessButton)
                    .menuIndicator(.hidden)
                    .fixedSize()
                    .disabled(library.draft == nil)
                    .help("Share as a link or a file; anyone with Radlet can add it")
                    Button(action: library.revealFolder) { Image(systemName: "folder") }
                        .help("Show in Finder")
                }
                .buttonStyle(.borderless)
                .padding(6)
            }
            .frame(width: 190)
            Divider()
            if let draft = library.draft {
                RecipeDraftView(draft: draft, library: library)
            } else {
                Text("Choose a loadout, or press + to make one.")
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .onAppear { library.refresh() }
    }
}

private struct RecipeDraftView: View {
    @ObservedObject var draft: RecipeDraft
    let library: RecipeLibrary

    var body: some View {
        VStack(spacing: 0) {
            Form {
                Section {
                    TextField("Name", text: $draft.name)
                    TextField("Description", text: $draft.summary, prompt: Text("Optional"))
                }
                .disabled(draft.builtin)
                Section("Steps, in order") {
                    ForEach(Array(draft.steps.enumerated()), id: \.element.id) { index, step in
                        StepEditor(step: step, index: index, count: draft.steps.count, locked: draft.builtin,
                                   move: { draft.move(step, by: $0) }, remove: { draft.remove(step) })
                    }
                    if !draft.builtin {
                        Menu("Add Step") {
                            ForEach(Array(StepText.templates.enumerated()), id: \.offset) { _, template in
                                Button(template.title) { draft.add(template.step) }
                            }
                        }
                        .fixedSize()
                        .disabled(draft.steps.count >= RecipeCheck.maxSteps)
                    }
                }
            }
            .formStyle(.grouped)
            Divider()
            HStack(spacing: 8) {
                Text(noteText)
                    .font(.caption)
                    .foregroundStyle(noteIsProblem ? Color.red : Color.secondary)
                    .lineLimit(2)
                Spacer()
                if draft.builtin {
                    Button("Duplicate to Edit") { library.duplicate() }
                } else {
                    Button("Revert") { library.revert() }
                        .disabled(!draft.dirty || draft.saving)
                    Button("Save") { library.save() }
                        .keyboardShortcut("s")
                        .disabled(!draft.dirty || draft.problem != nil || draft.saving)
                }
            }
            .padding(10)
        }
    }

    private var noteIsProblem: Bool {
        !draft.builtin && (draft.error != nil || draft.problem != nil)
    }

    private var noteText: String {
        if draft.builtin { return "Built-in loadouts can't be changed. Duplicate one to make your own." }
        if let e = draft.error { return e }
        if let p = draft.problem { return p }
        if draft.saving { return "Saving…" }
        if draft.dirty { return draft.path == nil ? "Not saved yet." : "Unsaved changes." }
        return draft.path.map { "Saved as \($0.lastPathComponent). Hold Control while dragging to use it." } ?? ""
    }
}

private struct StepEditor: View {
    @ObservedObject var step: StepModel
    let index: Int
    let count: Int
    let locked: Bool
    let move: (Int) -> Void
    let remove: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Text("\(index + 1).").foregroundStyle(.secondary).monospacedDigit()
                Text(step.title).fontWeight(.medium).lineLimit(1)
                Spacer()
                if !locked {
                    Button { move(-1) } label: { Image(systemName: "arrow.up") }
                        .disabled(index == 0)
                        .help("Move up")
                    Button { move(1) } label: { Image(systemName: "arrow.down") }
                        .disabled(index == count - 1)
                        .help("Move down")
                    Button(action: remove) { Image(systemName: "trash") }
                        .help("Remove this step")
                }
            }
            .buttonStyle(.borderless)
            if !locked && step.fields.hasOptions {
                options
            }
            if !locked, let problem = step.problem {
                Text(problem).font(.caption).foregroundStyle(.red)
            }
        }
        .padding(.vertical, 2)
    }

    @ViewBuilder private var options: some View {
        switch step.fields.op {
        case "convert":
            HStack {
                Picker("To", selection: $step.fields.format) {
                    ForEach(step.fields.formatChoices, id: \.self) { f in Text(StepText.formatName(f)).tag(f) }
                }
                TextField("Quality", text: $step.fields.quality, prompt: Text("Usual"))
                    .frame(width: 130)
            }
        case "resize":
            HStack {
                Picker("Size", selection: $step.fields.sizeMode) {
                    ForEach(StepFields.SizeMode.allCases) { m in Text(m.title).tag(m) }
                }
                TextField("", text: $step.fields.size)
                    .multilineTextAlignment(.trailing)
                    .frame(width: 70)
                Text(step.fields.sizeMode.unit).foregroundStyle(.secondary)
            }
        case "rotate":
            Picker("Turn", selection: $step.fields.degrees) {
                ForEach(StepFields.angles, id: \.self) { d in Text("\(d)°").tag(d) }
            }
            .pickerStyle(.segmented)
        case "compress":
            Picker("Level", selection: $step.fields.level) {
                Text("Smart").tag("smart")
                Text("Light").tag("light")
                Text("Balanced").tag("balanced")
                Text("Strong").tag("strong")
            }
            .pickerStyle(.segmented)
        case "watermark":
            TextField("Text", text: $step.fields.text)
            HStack {
                Text("Strength")
                Slider(value: $step.fields.opacity, in: 0.05...1)
                Text("\(Int((step.fields.opacity * 100).rounded()))%")
                    .monospacedDigit()
                    .frame(width: 40, alignment: .trailing)
            }
        case "page_numbers":
            HStack {
                Picker("Style", selection: $step.fields.numberStyle) {
                    ForEach(ToolAction.NumberStyle.allCases) { s in Text(s.example).tag(s) }
                }
                Picker("Where", selection: $step.fields.numberPosition) {
                    ForEach(ToolAction.NumberPosition.allCases) { p in Text(Self.positionName(p)).tag(p) }
                }
            }
        case "trim":
            HStack {
                TextField("From", text: $step.fields.start, prompt: Text("0:00"))
                TextField("To", text: $step.fields.end, prompt: Text("the end"))
            }
        case "remove_background":
            Toggle("Trim the picture to the subject", isOn: $step.fields.crop)
        case "trace":
            HStack {
                Toggle("Keep the colours", isOn: $step.fields.traceColor)
                Picker("Detail", selection: $step.fields.traceDetail) {
                    ForEach(StepText.TraceDetail.allCases) { d in Text(d.title).tag(d) }
                }
            }
        case "transcribe":
            HStack {
                Picker("Save as", selection: $step.fields.subtitleFormat) {
                    Text("SRT").tag("srt")
                    Text("WebVTT").tag("vtt")
                    Text("Transcript").tag("txt")
                }
                TextField("Language", text: $step.fields.language, prompt: Text("This Mac's"))
                    .frame(width: 150)
            }
        default:
            EmptyView()
        }
    }

    private static func positionName(_ p: ToolAction.NumberPosition) -> String {
        switch p {
        case .topLeft: return "Top left"
        case .topCenter: return "Top center"
        case .topRight: return "Top right"
        case .bottomLeft: return "Bottom left"
        case .bottomCenter: return "Bottom center"
        case .bottomRight: return "Bottom right"
        }
    }
}
