import AppKit
import ServiceManagement
import SwiftUI
import UniformTypeIdentifiers
import RadletKit

/// Preferences, read where they're needed so changes apply immediately.
enum Prefs {
    private static var d: UserDefaults { .standard }

    static var outputChoice: OutputChoice {
        OutputChoice(rawValue: d.string(forKey: "outputChoice") ?? "") ?? .besideSource
    }

    static var customFolder: String? { d.string(forKey: "customFolder") }

    static var isolate: Bool { d.object(forKey: "isolate") as? Bool ?? true }

    /// Off by default: originals go to the Trash after a job.
    static var replaceOriginals: Bool { d.bool(forKey: "replaceOriginals") }

    static var gestureEnabled: Bool { d.object(forKey: "gestureEnabled") as? Bool ?? true }

    /// ⌃⌥V opens the wheel for Finder's selection.
    static var finderShortcut: Bool { d.object(forKey: "finderShortcut") as? Bool ?? true }

    static var excludedApps: [String] { d.stringArray(forKey: "excludedApps") ?? [] }
    /// Shortcuts (from the Shortcuts app) on the Control wheel, by name.
    static var wheelShortcuts: [String] {
        get { d.stringArray(forKey: "wheelShortcuts") ?? [] }
        set { d.set(newValue, forKey: "wheelShortcuts") }
    }

    private static var supportDir: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent("Library/Application Support")
    }

    /// Loadouts you save: TOML files (they were called recipes).
    static var loadoutsDir: URL { supportDir.appendingPathComponent("Radlet/Loadouts", isDirectory: true) }

    static var applicationSupport: URL { supportDir }

    /// Skins you make or download: CSS files (see RadletKit/Skin.swift).
    static var skinsDir: URL { SkinLibrary.defaultDirectory }

    /// The Dial unless another style is picked in Settings → Wheel. (Orbit
    /// and Bloom used to sit behind an "Advanced styles" switch; turning it
    /// off already reset this to the Dial, so its old key can be ignored.)
    static var wheelStyle: WheelStyle {
        WheelStyle(rawValue: d.string(forKey: "wheelStyle") ?? "") ?? .dial
    }

    static var wheelSlots: Int {
        let n = d.integer(forKey: "wheelSlots")
        return WheelSpec.slotCounts.contains(n) ? n : WheelSpec.defaultCount
    }

    static var haptics: Bool { d.object(forKey: "haptics") as? Bool ?? true }

    /// Bloom: seconds resting on a slot before its presets fan out.
    static var bloomDwell: Double {
        let v = d.double(forKey: "bloomDwell")
        return v > 0 ? min(max(v, 0.12), 0.6) : 0.24
    }

    static var skinID: String { d.string(forKey: "skin") ?? SkinLibrary.defaultID }

    /// The engine's `OutputLocation` for the current choice. Files from a
    /// temporary place (a copied image) save to Downloads rather than
    /// beside it.
    static func outputJSON(temporarySource: Bool = false) -> JSONValue {
        func directory(_ path: String) -> JSONValue {
            .object(["kind": .string("directory"), "path": .string(path)])
        }
        let downloads = FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask).first
        switch outputChoice {
        case .besideSource:
            if temporarySource, let downloads { return directory(downloads.path) }
            return .object(["kind": .string("beside_source")])
        case .downloads:
            return downloads.map { directory($0.path) } ?? .object(["kind": .string("beside_source")])
        case .custom:
            if let folder = customFolder, !folder.isEmpty { return directory(folder) }
            if temporarySource, let downloads { return directory(downloads.path) }
            return .object(["kind": .string("beside_source")])
        }
    }
}

struct SettingsView: View {
    @ObservedObject var state: AppState

    var body: some View {
        TabView {
            GeneralSettings()
                .tabItem { Label("General", systemImage: "gearshape") }
            WheelSettings()
                .tabItem { Label("Wheel", systemImage: "circle.circle") }
            OutputSettings()
                .tabItem { Label("Output", systemImage: "folder") }
            RecipesSettings()
                .tabItem { Label("Loadouts", systemImage: "list.bullet.rectangle") }
            WatchSettings()
                .tabItem { Label("Watch", systemImage: "eye") }
            AdvancedSettings(state: state)
                .tabItem { Label("Advanced", systemImage: "wrench.and.screwdriver") }
        }
        .frame(width: 720, height: 560)
    }
}

/// Launch-at-login state, shared by Settings and the welcome window.
///
/// Views in this app keep state in ObservableObjects rather than `@State`:
/// newer SDKs implement `@State` as a macro whose plugin ships only with
/// Xcode, so `@State` breaks builds that use the Command Line Tools.
@MainActor
final class LoginItem: ObservableObject {
    static let shared = LoginItem()

    /// Bound to toggles. Setting it registers or unregisters the login item,
    /// then snaps back to what macOS actually did.
    @Published var enabled: Bool = SMAppService.mainApp.status == .enabled {
        didSet {
            if enabled != oldValue, !syncing { apply(enabled) }
        }
    }
    @Published private(set) var error: String?
    private var syncing = false

    /// Turns it on once, for a new install. Quietly: if macOS refuses (say,
    /// Radlet is still running from the disk image), it just stays off and
    /// the switch shows that.
    func enableByDefault() {
        guard !enabled else { return }
        do {
            try SMAppService.mainApp.register()
        } catch {
            Log.app("login item not added: \(Log.code(error))")
        }
        syncing = true
        enabled = SMAppService.mainApp.status == .enabled
        syncing = false
    }

    private func apply(_ on: Bool) {
        do {
            if on {
                try SMAppService.mainApp.register()
            } else {
                try SMAppService.mainApp.unregister()
            }
            error = nil
        } catch let failure {
            error = "Couldn't change this: \(failure.localizedDescription). Move Radlet to Applications and try again."
        }
        let actual = SMAppService.mainApp.status == .enabled
        if actual != enabled {
            syncing = true
            enabled = actual
            syncing = false
        }
    }
}

/// Apps where the wheel is turned off, stored as bundle IDs.
@MainActor
final class ExcludedApps: ObservableObject {
    @Published var ids: [String] = Prefs.excludedApps
    @Published var selection: String?

    func add() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.application]
        panel.directoryURL = URL(fileURLWithPath: "/Applications")
        guard panel.runModal() == .OK, let url = panel.url,
              let id = Bundle(url: url)?.bundleIdentifier, !ids.contains(id) else { return }
        ids.append(id)
        save()
    }

    func removeSelected() {
        ids.removeAll { $0 == selection }
        selection = nil
        save()
    }

    func name(for bundleID: String) -> String {
        guard let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleID) else { return bundleID }
        return FileManager.default.displayName(atPath: url.path)
    }

    private func save() {
        UserDefaults.standard.set(ids, forKey: "excludedApps")
    }
}

private struct GeneralSettings: View {
    @AppStorage("gestureEnabled") private var gestureEnabled = true
    @AppStorage("finderShortcut") private var finderShortcut = true
    @ObservedObject private var login = LoginItem.shared
    @StateObject private var excluded = ExcludedApps()
    @ObservedObject private var updates = Updates.shared

    var body: some View {
        Form {
            Section {
                Toggle("Open the wheel while dragging files", isOn: $gestureEnabled)
                Text("Hold Shift to convert, Command for tools, Control for loadouts. Radlet never reads your keystrokes, so it needs no special permissions.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Toggle("Open the wheel for Finder's selection with \(FinderShortcut.keys) in Finder", isOn: $finderShortcut)
                    .onChange(of: finderShortcut) { _, on in FinderShortcut.shared.setEnabled(on) }
                Text("Select files in Finder and press the shortcut: no drag needed. It works only while Finder is in front, so other apps keep it. macOS asks once to let Radlet read Finder's selection.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Toggle("Start Radlet when I log in", isOn: $login.enabled)
                if let error = login.error {
                    Text(error).font(.caption).foregroundStyle(.red)
                }
            }
            if updates.isAvailable {
                Section("Updates") {
                    Toggle("Check for updates automatically", isOn: Binding(
                        get: { updates.checksAutomatically },
                        set: { updates.checksAutomatically = $0 }
                    ))
                    Text("Once a day. Radlet asks before installing anything, and checks each update's signature first.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    Button("Check Now") { updates.checkForUpdates() }
                        .disabled(!updates.canCheck)
                }
            }
            Section("Turn the wheel off in these apps") {
                List(excluded.ids, id: \.self, selection: $excluded.selection) { id in
                    Text(excluded.name(for: id))
                }
                .frame(minHeight: 90)
                HStack {
                    Button("Add App…") { excluded.add() }
                    Button("Remove") { excluded.removeSelected() }
                        .disabled(excluded.selection == nil)
                }
            }
        }
        .formStyle(.grouped)
    }
}

private struct OutputSettings: View {
    @AppStorage("outputChoice") private var choice = OutputChoice.besideSource.rawValue
    @AppStorage("customFolder") private var customFolder = ""
    @AppStorage("replaceOriginals") private var replaceOriginals = false

    var body: some View {
        Form {
            Section("Save results") {
                Picker("Location", selection: $choice) {
                    ForEach(OutputChoice.allCases) { option in
                        Text(option.title).tag(option.rawValue)
                    }
                }
                .pickerStyle(.radioGroup)
                if choice == OutputChoice.custom.rawValue {
                    HStack {
                        Text(customFolder.isEmpty ? "No folder chosen" : customFolder)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .foregroundStyle(customFolder.isEmpty ? .secondary : .primary)
                        Spacer()
                        Button("Choose…", action: chooseFolder)
                    }
                }
                Text("Radlet never overwrites a file. If a name is taken, it adds a number, like \"photo 2.jpg\".")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Section("Replace originals") {
                Toggle("Move originals to the Trash after a job", isOn: $replaceOriginals)
                Text("For files saved next to their original. A result in the same format takes the original's name, so \"photo compressed.jpg\" becomes \"photo.jpg\". Undo puts the originals back.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .formStyle(.grouped)
    }

    private func chooseFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        if panel.runModal() == .OK, let url = panel.url {
            customFolder = url.path
        }
    }
}

private struct AdvancedSettings: View {
    @ObservedObject var state: AppState
    @ObservedObject private var cli = CLIInstaller.shared
    @AppStorage("isolate") private var isolate = true

    var body: some View {
        Form {
            Section {
                Toggle("Run each file in a separate process", isOn: $isolate)
                Text("Recommended. A damaged file can then only fail itself, never freeze Radlet or stop the rest of a batch.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Section("Wheel") {
                Text("When not everything fits, the wheel keeps the slots you use most, each in the same place. Styles and skins are in the Wheel tab; loadouts in the Loadouts tab.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                HStack {
                    Button("Forget Most Used") { Usage.shared.reset() }
                    Button("Open Loadouts Folder") { state.openRecipesFolder() }
                }
            }
            Section("AI agents (MCP)") {
                Text("Let Claude and other AI apps convert, compress, clean and check files with Radlet, on this Mac. Add Radlet as a local MCP server in the app's settings:")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Text(MCPSetup.desktopConfig)
                    .font(.system(.caption, design: .monospaced))
                    .textSelection(.enabled)
                HStack {
                    Button("Copy for Claude Desktop") { MCPSetup.copy(MCPSetup.desktopConfig) }
                    Button("Copy for Claude Code") { MCPSetup.copy(MCPSetup.claudeCode) }
                }
            }
            Section("Command line and Shortcuts") {
                Text("Use Radlet from Terminal, or from a Shortcuts \"Run Shell Script\" action, for example: radlet convert --to jpeg photo.heic")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                HStack {
                    Button("Install Command-Line Tool…") { cli.install() }
                    Text(cli.status).font(.caption).foregroundStyle(.secondary)
                }
            }
            Section("Engine") {
                Text(state.engineStatus).font(.callout)
                Button("Check Again") { Task { await state.checkEngine() } }
            }
            Section("Open source") {
                Text(OpenSource.notice)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                HStack {
                    Button("Show Licences") { OpenSource.showLicences() }
                    if let source = FFmpegInstaller.pinned?.releasePage {
                        Link("FFmpeg Source", destination: source)
                    }
                }
            }
        }
        .formStyle(.grouped)
        .onAppear { cli.refresh() }
    }
}

/// What Radlet owes the open-source projects it uses: a notice users can
/// see, and the licences that ship inside the app.
@MainActor
enum OpenSource {
    static var notice: String {
        var text = "Radlet includes open-source software, such as Sparkle for updates and PDFium for PDF pages; "
            + "their licences are inside the app. For more audio and video it uses FFmpeg (ffmpeg.org), "
            + "a separate program."
        if let pin = FFmpegInstaller.pinned {
            text += " The FFmpeg Radlet downloads on a Mac without Homebrew is FFmpeg \(pin.version), "
                + "licensed under the LGPL v2.1 or later. Its complete source, with the script that builds it, "
                + "is on its release page."
        }
        return text
    }

    /// Opens the folder of licences inside the app in Finder.
    static func showLicences() {
        guard let folder = Bundle.main.resourceURL?.appendingPathComponent("Licenses", isDirectory: true) else { return }
        NSWorkspace.shared.open(folder)
    }
}

/// Links /usr/local/bin/radlet to the helper inside the app, so Terminal and
/// Shortcuts can run it.
@MainActor
final class CLIInstaller: ObservableObject {
    static let shared = CLIInstaller()
    static let linkPath = "/usr/local/bin/radlet"
    @Published private(set) var status = ""

    private var helper: URL { Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/radlet") }

    func refresh() {
        let fm = FileManager.default
        if let dest = try? fm.destinationOfSymbolicLink(atPath: Self.linkPath), dest == helper.path {
            status = "Installed as radlet"
        } else if fm.fileExists(atPath: Self.linkPath) {
            status = "Another radlet is installed"
        } else {
            status = "Not installed"
        }
    }

    func install() {
        let fm = FileManager.default
        guard fm.isExecutableFile(atPath: helper.path) else {
            status = "This copy of Radlet has no helper to link to."
            return
        }
        let home = NSHomeDirectory()
        let path = Bundle.main.bundlePath
        guard path.hasPrefix("/Applications/") || path.hasPrefix(home + "/Applications/") else {
            status = "Move Radlet to Applications first."
            return
        }
        let existing = try? fm.destinationOfSymbolicLink(atPath: Self.linkPath)
        if existing != helper.path, fm.fileExists(atPath: Self.linkPath) || existing != nil {
            let alert = NSAlert()
            alert.messageText = "Replace the existing radlet command?"
            alert.informativeText = "\(Self.linkPath) already exists. Radlet will point it at this app instead."
            alert.addButton(withTitle: "Replace")
            alert.addButton(withTitle: "Cancel")
            guard alert.runModal() == .alertFirstButtonReturn else { return }
        }
        // Without a password when /usr/local/bin is writable (Homebrew on
        // Intel Macs makes it so), otherwise as an administrator.
        do {
            try fm.createDirectory(atPath: "/usr/local/bin", withIntermediateDirectories: true)
            if existing != nil || fm.fileExists(atPath: Self.linkPath) { try fm.removeItem(atPath: Self.linkPath) }
            try fm.createSymbolicLink(atPath: Self.linkPath, withDestinationPath: helper.path)
        } catch {
            let quoted = helper.path.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"")
            let source = "do shell script \"mkdir -p /usr/local/bin && ln -sf \" & quoted form of \"\(quoted)\" & \" \(Self.linkPath)\" with administrator privileges"
            var failure: NSDictionary?
            NSAppleScript(source: source)?.executeAndReturnError(&failure)
            if let failure, (failure[NSAppleScript.errorNumber] as? Int) != -128 {
                Log.app("cli install failed: \(failure[NSAppleScript.errorMessage] as? String ?? "unknown")")
            }
        }
        refresh()
        Log.app("cli install: \(status)")
    }
}
