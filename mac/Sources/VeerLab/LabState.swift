import AppKit
import SwiftUI
import VeerKit
import VeerUI

enum LabLayout: String, CaseIterable, Identifiable {
    case single, grid
    var id: String { rawValue }
}

enum LabBackdrop: String, CaseIterable, Identifiable {
    case wallpaper, plain, checker
    var id: String { rawValue }
    var title: String {
        switch self {
        case .wallpaper: return "Wallpaper"
        case .plain: return "Plain"
        case .checker: return "Transparent"
        }
    }
}

enum LabScheme: String, CaseIterable, Identifiable {
    case dark, light
    var id: String { rawValue }
    var colorScheme: ColorScheme { self == .dark ? .dark : .light }
}

enum LabZoom: String, CaseIterable, Identifiable {
    case fit, actual, double
    var id: String { rawValue }
    var title: String {
        switch self {
        case .fit: return "Fit"
        case .actual: return "Actual Size"
        case .double: return "200%"
        }
    }
}

/// Everything the lab shows: which story, which skin and your changes to
/// it, and the wheels on the stage.
@MainActor
final class LabState: ObservableObject {
    // MARK: What's on the stage

    @Published var storyID: String? = WheelStory.all[0].id {
        didSet {
            // The sidebar can clear its selection; keep showing something.
            guard storyID != nil else {
                storyID = oldValue
                return
            }
            if storyID != oldValue { rebuild() }
        }
    }
    @Published var count = WheelSpec.defaultCount {
        didSet { if count != oldValue { rebuild() } }
    }
    @Published var layout = LabLayout.single {
        didSet { if layout != oldValue { rebuild() } }
    }
    @Published var backdrop = LabBackdrop.wallpaper
    @Published var scheme = LabScheme.dark
    @Published var zoom = LabZoom.fit
    @Published var reduceMotion = false {
        didSet { if reduceMotion != oldValue { restyle() } }
    }
    /// Springs and stagger four times slower, to study the motion.
    @Published var slowMotion = false {
        didSet {
            if slowMotion != oldValue {
                restyle()
                if touring { startTour() }
            }
        }
    }
    @Published var touring = false {
        didSet { if touring != oldValue { touring ? startTour() : stopTour() } }
    }
    @Published var showInspector = true
    @Published private(set) var cells: [LabCell] = []

    var story: WheelStory { WheelStory.find(storyID) }

    // MARK: Skins

    let skinsDir: URL
    @Published private(set) var files: [SkinFile] = SkinLibrary.builtIns
    @Published var skinID = SkinLibrary.defaultID {
        didSet {
            guard skinID != oldValue else { return }
            // Changes to the skin you leave wait for you to come back.
            drafts[oldValue] = edited == original ? nil : edited
            loadSelected()
        }
    }
    /// The selected skin as saved.
    @Published private(set) var original = Skin()
    /// The selected skin with your changes.
    @Published private(set) var edited = Skin() {
        didSet {
            guard edited != oldValue else { return }
            restyle()
            markChanged()
        }
    }
    /// The name a saved copy gets.
    @Published var newName = "My Glass"
    /// The selected file changed on disk while you had changes of your own.
    @Published private(set) var changedOnDisk = false
    /// The last thing that happened: saved, copied, or what went wrong.
    @Published private(set) var status: String?

    let fields: [TokenField]
    /// Unsaved changes to skins other than the selected one.
    private var drafts: [String: Skin] = [:]
    private var signature: [String: Date]?
    private var diskTimer: Timer?
    private var tourTimer: Timer?

    init(skinsDir: URL) {
        self.skinsDir = skinsDir
        fields = SkinCSS.editable.map { TokenField($0) }
        for field in fields {
            field.apply = { [weak self] token, text in
                guard let self else { return (nil, Skin()) }
                return self.apply(token, text)
            }
        }
        reloadFiles()
        loadSelected()
        rebuild()
        diskTimer = Timer.scheduledTimer(withTimeInterval: 1.5, repeats: true) { [weak self] t in
            guard let self else {
                t.invalidate()
                return
            }
            MainActor.assumeIsolated { self.reloadFiles() }
        }
    }

    var userFiles: [SkinFile] { files.filter { !$0.skin.isBuiltIn } }
    var builtIns: [SkinFile] { files.filter { $0.skin.isBuiltIn } }
    var selectedFile: SkinFile? { files.first { $0.id == skinID } }

    func name(of id: String) -> String {
        files.first { $0.id == id }?.skin.name ?? id
    }

    var changeCount: Int { fields.filter(\.changed).count }

    func isEdited(_ id: String) -> Bool {
        id == skinID ? changeCount > 0 : drafts[id] != nil
    }

    /// Where skins are saved, and who else sees them.
    var folderNote: String {
        let path = (skinsDir.path as NSString).abbreviatingWithTildeInPath
        if skinsDir.standardizedFileURL == SkinLibrary.defaultDirectory.standardizedFileURL {
            return "Skins folder: \(path). Veer lists every skin in it in Settings, and the lab picks up changes as you save."
        }
        return "Skins folder: \(path), from --skins. Veer itself reads skins from its own folder."
    }

    /// Your changes as a skin file, ready to save or paste.
    var css: String {
        var copy = edited
        copy.name = newName.trimmingCharacters(in: .whitespaces).isEmpty ? "My Skin" : newName
        copy.author = ""
        return copy.compactCSS()
    }

    // MARK: Editing

    private func apply(_ token: SkinCSS.Token, _ text: String) -> (problem: String?, skin: Skin) {
        var skin = edited
        let problem = token.apply(text, to: &skin)
        edited = skin
        return (problem, edited)
    }

    func reset(_ field: TokenField) {
        var skin = edited
        _ = field.token.apply(field.token.value(in: original), to: &skin)
        edited = skin
        field.show(edited)
    }

    func revertAll() {
        edited = original
        drafts[skinID] = nil
        changedOnDisk = false
        for field in fields { field.show(edited) }
        status = nil
    }

    private func markChanged() {
        for field in fields {
            let changed = field.token.value(in: edited) != field.token.value(in: original)
            if field.changed != changed { field.changed = changed }
        }
    }

    /// Shows the selected skin, with any changes you left on it.
    private func loadSelected() {
        guard let file = selectedFile else {
            skinID = SkinLibrary.defaultID
            return
        }
        original = file.skin
        edited = drafts.removeValue(forKey: file.id) ?? file.skin
        newName = file.skin.isBuiltIn ? "My \(file.skin.name)" : "\(file.skin.name) copy"
        changedOnDisk = false
        status = nil
        for field in fields { field.show(edited) }
        markChanged()
        // The skin you left goes back to how it's saved, in every layout.
        restyle()
    }

    /// Takes the file as it is on disk now, dropping your changes.
    func loadFromDisk() {
        drafts[skinID] = nil
        edited = original
        loadSelected()
    }

    // MARK: Files

    /// Rereads the Skins folder if anything in it changed.
    func reloadFiles(force: Bool = false) {
        let now = SkinLibrary.signature(directory: skinsDir)
        guard force || now != signature else { return }
        signature = now
        let before = selectedFile
        files = SkinLibrary.builtIns + SkinLibrary.load(directory: skinsDir)
        guard let after = selectedFile else {
            // Its file has gone.
            skinID = SkinLibrary.defaultID
            if layout == .grid { rebuild() }
            return
        }
        if before != nil, after != before {
            if edited == original {
                loadSelected()
            } else {
                // Keep your changes; Revert All now goes to the new file.
                original = after.skin
                changedOnDisk = true
                markChanged()
            }
        }
        // Drafts of skins whose files have gone.
        for id in drafts.keys where !files.contains(where: { $0.id == id }) { drafts[id] = nil }
        if layout == .grid { rebuild() }
    }

    func openFolder() {
        guard makeFolder() else { return }
        NSWorkspace.shared.open(skinsDir)
    }

    func revealSelected() {
        guard let url = selectedFile?.url else { return }
        NSWorkspace.shared.activateFileViewerSelecting([url])
    }

    func copyCSS() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(css, forType: .string)
        status = "Copied. Paste it into a .css file in the Skins folder."
    }

    /// Writes your changes as a new file in the Skins folder and selects it.
    func saveAsNew() {
        guard makeFolder() else { return }
        let name = newName.trimmingCharacters(in: .whitespaces)
        let url = SkinLibrary.newFileURL(in: skinsDir, name: name.isEmpty ? "My Skin" : name)
        do {
            try css.write(to: url, atomically: true, encoding: .utf8)
        } catch {
            status = "Couldn't save: \(error.localizedDescription)"
            return
        }
        let saved = skinID
        reloadFiles(force: true)
        skinID = "file:\(url.lastPathComponent)"
        // The changes live on in the new file, not as a draft of the old skin.
        drafts[saved] = nil
        restyle()
        status = "Saved \(url.lastPathComponent) in the Skins folder."
    }

    private func makeFolder() -> Bool {
        do {
            try FileManager.default.createDirectory(at: skinsDir, withIntermediateDirectories: true)
            return true
        } catch {
            status = "Couldn't make the Skins folder: \(error.localizedDescription)"
            return false
        }
    }

    // MARK: The stage

    /// The skin as drawn: your changes, slowed down if asked.
    private func drawn(_ skin: Skin) -> Skin {
        guard slowMotion else { return skin }
        var slow = skin
        slow.springResponse = skin.springResponse * 4
        slow.stagger = skin.stagger * 4
        return slow
    }

    private func skin(for cellID: String) -> Skin {
        if layout == .single || cellID == skinID { return edited }
        return drafts[cellID] ?? files.first { $0.id == cellID }?.skin ?? edited
    }

    /// New wheels for the story: after a change of story, slots or layout.
    private func rebuild() {
        let story = self.story
        let ids = layout == .single ? ["single"] : files.map(\.id)
        cells = ids.map { id in
            LabCell(id: id, story: story, count: count, skin: drawn(skin(for: id)), reduceMotion: reduceMotion)
        }
    }

    /// The same wheels, restyled: after a skin edit or a motion setting.
    private func restyle() {
        for cell in cells {
            cell.restyle(skin: drawn(skin(for: cell.id)), reduceMotion: reduceMotion)
        }
    }

    func replay() {
        for cell in cells { cell.replay() }
    }

    private func startTour() {
        tourTimer?.invalidate()
        tourTimer = Timer.scheduledTimer(withTimeInterval: slowMotion ? 3.2 : 1.3, repeats: true) { [weak self] t in
            guard let self else {
                t.invalidate()
                return
            }
            MainActor.assumeIsolated {
                for cell in self.cells { cell.advanceTour() }
            }
        }
        for cell in cells { cell.advanceTour() }
    }

    private func stopTour() {
        tourTimer?.invalidate()
        tourTimer = nil
        for cell in cells { cell.endTour() }
    }
}
