import AppKit
import SwiftUI
import RadletKit
import RadletUI

/// The built-in skins plus the CSS files in the Skins folder.
@MainActor
final class SkinStore: ObservableObject {
    static let shared = SkinStore()

    @Published private(set) var files: [SkinFile] = SkinLibrary.builtIns
    @Published var selectedID: String = Prefs.skinID {
        didSet { UserDefaults.standard.set(selectedID, forKey: "skin") }
    }
    private var signature: [String: Date]?

    var builtIns: [SkinFile] { files.filter { $0.skin.isBuiltIn } }
    var userFiles: [SkinFile] { files.filter { !$0.skin.isBuiltIn } }
    var selected: SkinFile? { files.first { $0.id == selectedID } }
    /// A skin that has gone missing falls back to the default, Glass.
    var current: Skin { selected?.skin ?? SkinLibrary.builtIns[0].skin }

    /// Rereads the Skins folder if any file in it changed. Cheap enough to
    /// run each time the wheel opens.
    func refreshIfChanged() {
        let dir = Prefs.skinsDir
        let now = SkinLibrary.signature(directory: dir)
        guard now != signature else { return }
        signature = now
        files = SkinLibrary.builtIns + SkinLibrary.load(directory: dir)
        // A skin whose file has gone: back to the default, so the picker
        // shows what the wheel uses.
        if selected == nil { selectedID = SkinLibrary.defaultID }
        let mine = userFiles
        if !mine.isEmpty {
            let troubled = mine.filter { !$0.issues.isEmpty }.count
            Log.app("skins: \(mine.count) in the folder\(troubled > 0 ? ", \(troubled) with problems" : "")")
        }
    }

    func reload() {
        signature = nil
        refreshIfChanged()
    }

    func openFolder() {
        guard ensureFolder() else { return }
        NSWorkspace.shared.open(Prefs.skinsDir)
    }

    /// Writes the current skin as a new, complete file in the Skins folder,
    /// selects it and shows it in Finder, ready to edit.
    func makeNew() {
        guard ensureFolder() else { return }
        let skin = current
        var copy = skin
        copy.name = String((skin.isBuiltIn ? "My \(skin.name)" : "\(skin.name) copy").prefix(40))
        copy.author = ""
        let url = SkinLibrary.newFileURL(in: Prefs.skinsDir, name: copy.name)
        do {
            try copy.css(base: skin.base).write(to: url, atomically: true, encoding: .utf8)
        } catch {
            Log.app("skins: couldn't write a new skin: \(Log.code(error))")
            return
        }
        reload()
        selectedID = "file:\(url.lastPathComponent)"
        NSWorkspace.shared.activateFileViewerSelecting([url])
    }

    /// Shares the chosen skin (one of yours) as a link or a file: just its
    /// name and the tokens that differ from its base.
    func share(asFile: Bool) {
        guard let file = selected, !file.skin.isBuiltIn,
              let item = SharedItem(kind: .skin, text: file.skin.compactCSS()) else { return }
        if asFile {
            ShareExport.saveFile(item, name: file.skin.name)
        } else {
            ShareExport.copyLink(item, name: file.skin.name)
        }
    }

    func editSelected() {
        guard let url = selected?.url else { return }
        NSWorkspace.shared.open(url)
    }

    @discardableResult
    private func ensureFolder() -> Bool {
        do {
            try FileManager.default.createDirectory(at: Prefs.skinsDir, withIntermediateDirectories: true)
            return true
        } catch {
            Log.app("skins: couldn't create the folder: \(Log.code(error))")
            return false
        }
    }
}

struct WheelSettings: View {
    @AppStorage("wheelStyle") private var style = WheelStyle.dial.rawValue
    @AppStorage("wheelSlots") private var slots = WheelSpec.defaultCount
    @AppStorage("haptics") private var haptics = true
    @AppStorage("bloomDwell") private var dwell = 0.24
    @ObservedObject private var skins = SkinStore.shared
    @StateObject private var preview = WheelPreviewModel()

    private var chosen: WheelStyle { WheelStyle(rawValue: style) ?? .dial }

    private var count: Int { WheelSpec.slotCounts.contains(slots) ? slots : WheelSpec.defaultCount }

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            Form {
                styleSection
                skinSection
            }
            .formStyle(.grouped)
            VStack(spacing: 10) {
                WheelPreview(model: preview.model)
                Text("\(chosen.title) · \(skins.current.name)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .frame(width: 250)
            .padding(.top, 24)
            .padding(.trailing, 12)
        }
        .onAppear {
            skins.refreshIfChanged()
            if WheelStyle(rawValue: style) == nil { style = WheelStyle.dial.rawValue }
            if !WheelSpec.slotCounts.contains(slots) { slots = WheelSpec.defaultCount }
            refreshPreview()
            preview.start()
        }
        .onDisappear { preview.stop() }
        .onChange(of: style) { _, _ in refreshPreview() }
        .onChange(of: slots) { _, _ in refreshPreview() }
        .onChange(of: skins.selectedID) { _, _ in refreshPreview() }
        .onChange(of: skins.files) { _, _ in refreshPreview() }
    }

    private func refreshPreview() {
        preview.update(style: chosen, count: count, skin: skins.current)
    }

    private var styleSection: some View {
        Section("Style") {
            WheelStyleChoices()
            Picker("Slots", selection: $slots) {
                ForEach(WheelSpec.slotCounts, id: \.self) { n in Text("\(n)").tag(n) }
            }
            .pickerStyle(.segmented)
            Text("Eight puts every slot on a quick direction. The rest go behind “More…”.")
                .font(.caption)
                .foregroundStyle(.secondary)
            if chosen.isAdvanced {
                LabeledContent(chosen == .bloom ? "Open presets after" : "Fold presets back after") {
                    HStack {
                        Slider(value: $dwell, in: 0.12...0.6)
                        Text("\(Int((dwell * 1000).rounded())) ms")
                            .monospacedDigit()
                            .frame(width: 52, alignment: .trailing)
                    }
                }
                Text(chosen == .bloom
                     ? "How long to rest on a slot before its presets fan out. Open presets stay out until you rest on another slot."
                     : "Pushing into a slot widens its ring to show every preset. It stays wide until you rest on another slot for this long.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Toggle("Haptic feedback", isOn: $haptics)
            Text("A light tap on a Force Touch trackpad each time you cross into a slot.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private var skinSection: some View {
        Section("Skin") {
            Picker("Skin", selection: $skins.selectedID) {
                ForEach(skins.builtIns) { f in Text(f.skin.name).tag(f.id) }
                if !skins.userFiles.isEmpty {
                    Divider()
                    ForEach(skins.userFiles) { f in
                        Text(f.issues.isEmpty ? f.skin.name : "\(f.skin.name) (needs a fix)").tag(f.id)
                    }
                }
            }
            if let file = skins.selected, !file.issues.isEmpty {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(Array(file.issues.prefix(6).enumerated()), id: \.offset) { _, issue in
                        Text(issue.description)
                    }
                    if file.issues.count > 6 { Text("…and \(file.issues.count - 6) more") }
                }
                .font(.caption)
                .foregroundStyle(.orange)
                .textSelection(.enabled)
            }
            HStack {
                Button("New Skin from This") { skins.makeNew() }
                Button("Edit") { skins.editSelected() }
                    .disabled(skins.selected?.url == nil)
                Button("Open Skins Folder") { skins.openFolder() }
                Button("Reload") { skins.reload() }
                Menu("Share") {
                    Button("Copy Link") { skins.share(asFile: false) }
                    Button("Save as File…") { skins.share(asFile: true) }
                }
                .fixedSize()
                .disabled(skins.selected.map(\.skin.isBuiltIn) ?? true)
                .help("Share this skin as a link or a file; anyone with Radlet can add it")
            }
            Text("A skin is a CSS file of custom properties, such as --accent: #C5F05C. Change one in any text editor and save; Radlet picks it up the next time the wheel opens. Skins shared with you open in Radlet from a link or a .radletskin file.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// The wheel, shrunk to fit Settings and the setup guide.
struct WheelPreview: View {
    @ObservedObject var model: WheelModel

    var body: some View {
        let side = 236.0
        let scale = side / model.spec.size
        WheelView(model: model)
            .scaleEffect(scale)
            .frame(width: side, height: side)
            .allowsHitTesting(false)
            .accessibilityHidden(true)
    }
}

/// The three wheel styles as radio rows, Orbit and Bloom under an
/// "Advanced" header. Used by Settings and the setup guide; inside a Form
/// each row is its own line.
struct WheelStyleChoices: View {
    @AppStorage("wheelStyle") private var style = WheelStyle.dial.rawValue
    /// Marks the default, the Dial, as "(Recommended)" (the setup guide).
    private let recommended: Bool

    init(recommended: Bool = false) {
        self.recommended = recommended
    }

    private var chosen: WheelStyle { WheelStyle(rawValue: style) ?? .dial }

    var body: some View {
        Group {
            ForEach(WheelStyle.allCases.filter { !$0.isAdvanced }) { row($0) }
            VStack(alignment: .leading, spacing: 2) {
                Text("Advanced")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary)
                Text("A second level of presets for each slot, such as a quality or a size limit. Slots stay in the same places in every style.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .accessibilityElement(children: .combine)
            .accessibilityAddTraits(.isHeader)
            ForEach(WheelStyle.allCases.filter(\.isAdvanced)) { row($0) }
        }
    }

    private func row(_ s: WheelStyle) -> some View {
        RadioRow(title: recommended && s == .dial ? "\(s.title) (Recommended)" : s.title,
                 detail: s.summary, on: chosen == s) { style = s.rawValue }
    }
}

/// One choice of several, picked with a single click: a radio button, a
/// name and a line on what it does.
struct RadioRow: View {
    let title: String
    let detail: String
    let on: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: on ? "largecircle.fill.circle" : "circle")
                    .foregroundStyle(on ? Color.accentColor : Color.secondary)
                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                    Text(detail)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 0)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}
