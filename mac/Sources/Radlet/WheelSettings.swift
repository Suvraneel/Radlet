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
    @AppStorage("advancedStyles") private var advanced = false
    @AppStorage("wheelSlots") private var slots = WheelSpec.defaultCount
    @AppStorage("haptics") private var haptics = true
    @AppStorage("bloomDwell") private var dwell = 0.24
    @ObservedObject private var skins = SkinStore.shared
    @StateObject private var preview = WheelPreviewModel()

    private var chosen: WheelStyle {
        let s = WheelStyle(rawValue: style) ?? .dial
        return s.isAdvanced && !advanced ? .dial : s
    }

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
            if !advanced, (WheelStyle(rawValue: style) ?? .dial).isAdvanced { style = WheelStyle.dial.rawValue }
            if WheelStyle(rawValue: style) == nil { style = WheelStyle.dial.rawValue }
            if !WheelSpec.slotCounts.contains(slots) { slots = WheelSpec.defaultCount }
            refreshPreview()
            preview.start()
        }
        .onDisappear { preview.stop() }
        .onChange(of: style) { _, _ in refreshPreview() }
        .onChange(of: advanced) { _, on in
            if !on, (WheelStyle(rawValue: style) ?? .dial).isAdvanced { style = WheelStyle.dial.rawValue }
            refreshPreview()
        }
        .onChange(of: slots) { _, _ in refreshPreview() }
        .onChange(of: skins.selectedID) { _, _ in refreshPreview() }
        .onChange(of: skins.files) { _, _ in refreshPreview() }
    }

    private func refreshPreview() {
        preview.update(style: chosen, count: count, skin: skins.current)
    }

    private var styleSection: some View {
        Section("Style") {
            Picker("Style", selection: $style) {
                ForEach(WheelStyle.allCases.filter { advanced || !$0.isAdvanced }) { s in
                    Text(s.title).tag(s.rawValue)
                }
            }
            .pickerStyle(.radioGroup)
            Text(chosen.summary)
                .font(.caption)
                .foregroundStyle(.secondary)
            Toggle("Advanced styles", isOn: $advanced)
            Text("Orbit and Bloom give each slot a second level of presets, such as a quality or a size limit. Slots stay in the same places in every style.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Picker("Slots", selection: $slots) {
                ForEach(WheelSpec.slotCounts, id: \.self) { n in Text("\(n)").tag(n) }
            }
            .pickerStyle(.segmented)
            Text("Eight puts every slot on a quick direction. The rest go behind “More…”.")
                .font(.caption)
                .foregroundStyle(.secondary)
            if chosen == .bloom {
                LabeledContent("Open presets after") {
                    HStack {
                        Slider(value: $dwell, in: 0.12...0.6)
                        Text("\(Int((dwell * 1000).rounded())) ms")
                            .monospacedDigit()
                            .frame(width: 52, alignment: .trailing)
                    }
                }
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
            }
            Text("A skin is a CSS file of custom properties, such as --accent: #C5F05C. Change one in any text editor and save; Radlet picks it up the next time the wheel opens.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// The wheel, shrunk to fit Settings.
private struct WheelPreview: View {
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
