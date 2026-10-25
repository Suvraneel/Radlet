import AppKit
import ImageIO
import SwiftUI
import UniformTypeIdentifiers
import RadletKit
import RadletUI

/// The tutorial's steps, posted by AppState as they really happen.
enum Practice: String, CaseIterable {
    /// A convert chosen from a wheel opened by a Finder drag.
    case convert
    /// A tool chosen the same way.
    case tools
    /// ⌃⌥V opened the wheel for Finder's selection.
    case shortcut

    static let notification = Notification.Name("RadletPracticeStep")

    static func post(_ step: Practice) {
        NotificationCenter.default.post(name: notification, object: nil, userInfo: ["step": step.rawValue])
    }
}

/// The setup guide: shown on first launch, and from the menu bar afterwards.
@MainActor
final class WelcomeController {
    private var window: NSWindow?
    private let model = OnboardingModel()
    private static let shownKey = "welcomeShown"

    /// Shows the guide once per install.
    func showIfFirstLaunch() {
        guard !UserDefaults.standard.bool(forKey: Self.shownKey) else { return }
        UserDefaults.standard.set(true, forKey: Self.shownKey)
        // On unless the person turns it off in the guide (step 4).
        LoginItem.shared.enableByDefault()
        show()
    }

    func show() {
        if window == nil {
            let host = NSHostingController(rootView: OnboardingView(model: model, onDone: { [weak self] in
                self?.window?.close()
            }))
            let w = NSWindow(contentViewController: host)
            w.title = "Set Up Radlet"
            w.styleMask = [.titled, .closable, .fullSizeContentView]
            w.titlebarAppearsTransparent = true
            w.isReleasedWhenClosed = false
            w.center()
            window = w
            model.onPageChange = { [weak self] page in
                if page == .practice { self?.moveAside() }
                self?.updateLevel()
            }
            NotificationCenter.default.addObserver(forName: NSWindow.willCloseNotification, object: w, queue: .main) {
                [weak self] _ in
                MainActor.assumeIsolated { self?.model.stop() }
            }
            // Practising happens in Finder: while Finder is in front, keep the
            // checklist floating in view. While Radlet is (a tool window,
            // Settings), let its own windows come first.
            for name in [NSApplication.didResignActiveNotification, NSApplication.didBecomeActiveNotification] {
                NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                    MainActor.assumeIsolated { self?.updateLevel() }
                }
            }
        }
        if window?.isVisible != true { model.begin() }
        NSApp.activate()
        window?.makeKeyAndOrderFront(nil)
    }

    private func updateLevel() {
        guard let window else { return }
        let float = window.isVisible && model.page == .practice && !NSApp.isActive
        window.level = float ? .floating : .normal
    }

    /// To the right edge of the screen, so the Finder window stays clear.
    private func moveAside() {
        guard let window, let screen = window.screen ?? NSScreen.main else { return }
        let area = screen.visibleFrame
        window.setFrameOrigin(NSPoint(x: area.maxX - window.frame.width - 16,
                                      y: area.midY - window.frame.height / 2))
    }
}

/// What the guide shows, and what it has seen the person do.
@MainActor
final class OnboardingModel: ObservableObject {
    enum Page: Int, CaseIterable {
        case welcome, wheel, results, shortcuts, media, practice

        var title: String {
            switch self {
            case .welcome: return "Welcome"
            case .wheel: return "Your wheel"
            case .results: return "Where results go"
            case .shortcuts: return "Shortcuts and startup"
            case .media: return "Audio and video"
            case .practice: return "Try it"
            }
        }
    }

    @Published var page: Page = .welcome
    /// Whether the engine found FFmpeg; nil while checking.
    @Published private(set) var ffmpeg: Bool?
    /// Why the answer above may not be the whole story.
    @Published private(set) var ffmpegNote: String?
    @Published private(set) var done: Set<Practice> = []
    @Published private(set) var practiceFolder: URL?
    @Published private(set) var practiceProblem: String?
    let preview = WheelPreviewModel()
    var onPageChange: ((Page) -> Void)?
    /// Only what's done while the guide is open counts.
    private var listening = false

    init() {
        NotificationCenter.default.addObserver(forName: Practice.notification, object: nil, queue: .main) {
            [weak self] note in
            guard let raw = note.userInfo?["step"] as? String, let step = Practice(rawValue: raw) else { return }
            MainActor.assumeIsolated {
                guard let self, self.listening else { return }
                self.done.insert(step)
            }
        }
    }

    /// From the first page, as the window opens.
    func begin() {
        listening = true
        done = []
        page = .welcome
        onPageChange?(page)
        SkinStore.shared.refreshIfChanged()
        refreshPreview()
        preview.start()
        checkFFmpeg(restart: false)
    }

    func stop() {
        listening = false
        preview.stop()
        page = .welcome
        onPageChange?(.welcome)
    }

    func go(_ delta: Int) {
        guard let next = Page(rawValue: page.rawValue + delta) else { return }
        page = next
        onPageChange?(next)
    }

    func refreshPreview() {
        preview.update(style: Prefs.wheelStyle, count: Prefs.wheelSlots, skin: SkinStore.shared.current)
    }

    /// `restart` makes the helper look again, after installing FFmpeg.
    func checkFFmpeg(restart: Bool) {
        ffmpeg = nil
        ffmpegNote = nil
        Task {
            do {
                if restart {
                    let answer = try await EngineClient.shared.refreshedHello()
                    ffmpeg = answer.hello.capabilities.ffmpeg
                    if !answer.restarted {
                        ffmpegNote = "Radlet is busy with a job, so it couldn't look again yet. Try once the job finishes."
                    }
                    await AppState.shared.checkEngine()
                } else {
                    ffmpeg = try await EngineClient.shared.hello().capabilities.ffmpeg
                }
            } catch {
                ffmpeg = false
                ffmpegNote = "Radlet's engine didn't answer (\(error.localizedDescription)), so it couldn't check."
            }
        }
    }

    func openPracticeFolder() {
        do {
            let made = try PracticeFolder.make()
            practiceFolder = made.folder
            practiceProblem = nil
            NSWorkspace.shared.activateFileViewerSelecting([made.first])
        } catch {
            practiceProblem = "Couldn't make the practice folder: \(error.localizedDescription)"
        }
    }
}

private struct OnboardingView: View {
    @ObservedObject var model: OnboardingModel
    var onDone: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            StepBar(page: model.page)
                .padding(.top, 30)
            Group {
                switch model.page {
                case .welcome: WelcomePage()
                case .wheel: WheelPage(model: model, preview: model.preview)
                case .results: ResultsPage()
                case .shortcuts: ShortcutsPage()
                case .media: MediaPage(model: model)
                case .practice: PracticePage(model: model)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .padding(.horizontal, 36)
            .padding(.vertical, 22)
            Divider()
            HStack {
                if model.page != .welcome {
                    Button("Back") { model.go(-1) }
                }
                Spacer()
                if model.page != .practice {
                    Button("Skip Setup", action: onDone)
                        .buttonStyle(.link)
                        .padding(.trailing, 8)
                }
                Button(primaryTitle) {
                    if model.page == .practice { onDone() } else { model.go(1) }
                }
                .keyboardShortcut(.defaultAction)
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 14)
        }
        .frame(width: 720, height: 560)
    }

    private var primaryTitle: String {
        switch model.page {
        case .welcome: return "Set Up Radlet"
        case .practice: return "Done"
        default: return "Continue"
        }
    }
}

/// Where the guide is: a dot per page, the current one stretched.
private struct StepBar: View {
    let page: OnboardingModel.Page

    var body: some View {
        HStack(spacing: 6) {
            ForEach(OnboardingModel.Page.allCases, id: \.self) { p in
                Capsule()
                    .fill(p.rawValue <= page.rawValue ? Color.accentColor : Color.primary.opacity(0.12))
                    .frame(width: p == page ? 22 : 8, height: 8)
            }
        }
        .animation(.spring(response: 0.35, dampingFraction: 0.8), value: page)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Step \(page.rawValue + 1) of \(OnboardingModel.Page.allCases.count): \(page.title)")
    }
}

private struct PageTitle: View {
    let title: String
    let detail: String

    init(_ title: String, _ detail: String) {
        self.title = title
        self.detail = detail
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(.title2.weight(.semibold))
                .accessibilityAddTraits(.isHeader)
            Text(detail)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.bottom, 6)
    }
}

// MARK: - Pages

private struct WelcomePage: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack(spacing: 14) {
                Image(systemName: "circle.circle")
                    .font(.system(size: 40, weight: .medium))
                    .foregroundStyle(Color.accentColor)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 2) {
                    Text("Welcome to Radlet").font(.largeTitle.weight(.semibold))
                    Text("A weapon wheel for your files.").font(.title3).foregroundStyle(.secondary)
                }
            }
            Text("Start dragging files in Finder, hold a key, and flick to what you want:")
                .font(.headline)
            VStack(alignment: .leading, spacing: 10) {
                KeyRow(key: "⇧ Shift", title: "Convert", detail: "JPEG, PNG, WebP, PDF, MP4, MP3 and more.")
                KeyRow(key: "⌘ Command", title: "Tools", detail: "Crop, compress, remove backgrounds, hide faces, translate and more.")
                KeyRow(key: "⌃ Control", title: "Loadouts", detail: "Your saved sets of steps, like Web-ready image.")
            }
            Text("Setup takes about a minute: pick how the wheel looks, choose where results go, then try it on some sample photos. Everything happens on your Mac.")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

private struct WheelPage: View {
    @ObservedObject var model: OnboardingModel
    /// Observed itself, so the wheel redraws when the style or skin changes.
    @ObservedObject var preview: WheelPreviewModel
    @ObservedObject private var skins = SkinStore.shared
    @AppStorage("wheelStyle") private var style = WheelStyle.dial.rawValue

    var body: some View {
        HStack(alignment: .top, spacing: 28) {
            VStack(alignment: .leading, spacing: 12) {
                PageTitle("Pick your wheel", "Change it any time in Settings → Wheel.")
                VStack(alignment: .leading, spacing: 10) {
                    WheelStyleChoices(recommended: true)
                }
                Picker("Skin", selection: $skins.selectedID) {
                    ForEach(skins.files) { f in Text(f.skin.name).tag(f.id) }
                }
                .frame(maxWidth: 280)
                .padding(.top, 6)
            }
            VStack(spacing: 10) {
                WheelPreview(model: preview.model)
                Text("\((WheelStyle(rawValue: style) ?? .dial).title) · \(skins.current.name)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .frame(width: 250)
            .padding(.top, 8)
        }
        .onAppear { model.refreshPreview() }
        .onChange(of: style) { _, _ in model.refreshPreview() }
        .onChange(of: skins.selectedID) { _, _ in model.refreshPreview() }
    }
}

private struct ResultsPage: View {
    @AppStorage("outputChoice") private var choice = OutputChoice.besideSource.rawValue
    @AppStorage("customFolder") private var customFolder = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            PageTitle("Where should results go?",
                      "Radlet never overwrites a file. If a name is taken, it adds a number, like “photo 2.jpg”.")
            RadioRow(title: "\(OutputChoice.besideSource.title) (Recommended)",
                     detail: "photo.heic becomes photo.jpg in the same folder, right where you were looking.",
                     on: choice == OutputChoice.besideSource.rawValue) { choice = OutputChoice.besideSource.rawValue }
            RadioRow(title: OutputChoice.downloads.title,
                     detail: "Everything Radlet makes lands in Downloads.",
                     on: choice == OutputChoice.downloads.rawValue) { choice = OutputChoice.downloads.rawValue }
            RadioRow(title: OutputChoice.custom.title,
                     detail: customFolder.isEmpty
                        ? (choice == OutputChoice.custom.rawValue
                            ? "No folder chosen yet, so results go next to the original."
                            : "One folder for every result.")
                        : customFolder,
                     on: choice == OutputChoice.custom.rawValue) { chooseFolder() }
            if choice == OutputChoice.custom.rawValue {
                Button("Choose Another Folder…", action: chooseFolder)
                    .padding(.leading, 26)
            }
        }
    }

    private func chooseFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        panel.prompt = "Use This Folder"
        if panel.runModal() == .OK, let url = panel.url {
            customFolder = url.path
            choice = OutputChoice.custom.rawValue
        }
    }
}

private struct ShortcutsPage: View {
    @AppStorage("finderShortcut") private var finderShortcut = true
    @AppStorage("haptics") private var haptics = true
    @ObservedObject private var login = LoginItem.shared

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            PageTitle("Shortcuts and startup", "You can change all of these later in Settings.")
            ToggleRow(isOn: $finderShortcut, title: "Open the wheel with \(FinderShortcut.keys) (Recommended)",
                      detail: "Select files in Finder and press \(FinderShortcut.keys): no drag needed.")
                .onChange(of: finderShortcut) { _, on in FinderShortcut.shared.setEnabled(on) }
            ToggleRow(isOn: $login.enabled, title: "Start Radlet when you log in (Recommended)",
                      detail: "So the wheel is there whenever you drag files. Radlet lives in the menu bar.")
            if let error = login.error {
                Text(error)
                    .font(.caption)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
            ToggleRow(isOn: $haptics, title: "Haptic feedback (Recommended)",
                      detail: "A light tap on a Force Touch trackpad as you cross into a slot.")
            Text("You can also right-click files in Finder and choose Services → Convert with Radlet.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

private struct MediaPage: View {
    @ObservedObject var model: OnboardingModel
    private static let command = "brew install ffmpeg"

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            PageTitle("Audio and video",
                      "Images, PDFs and documents work already. Audio and video need FFmpeg, a free tool that Radlet finds on its own once it's installed.")
            switch model.ffmpeg {
            case nil:
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Looking for FFmpeg…").foregroundStyle(.secondary)
                }
            case true?:
                Label("FFmpeg is installed: MP4, MOV, MP3, trimming, GIFs and loudness are ready.",
                      systemImage: "checkmark.circle.fill")
                    .foregroundStyle(.green)
            case false?:
                VStack(alignment: .leading, spacing: 12) {
                    Label("FFmpeg isn't installed yet.", systemImage: "info.circle")
                    Text("If you have Homebrew, paste this into Terminal:")
                    HStack(spacing: 10) {
                        Text(Self.command)
                            .font(.system(.body, design: .monospaced))
                            .textSelection(.enabled)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 6)
                            .background(RoundedRectangle(cornerRadius: 6).fill(Color.primary.opacity(0.07)))
                        Button("Copy") {
                            NSPasteboard.general.clearContents()
                            NSPasteboard.general.setString(Self.command, forType: .string)
                        }
                    }
                    HStack(spacing: 14) {
                        Button("Check Again") { model.checkFFmpeg(restart: true) }
                        Link("What's Homebrew?", destination: URL(string: "https://brew.sh")!)
                    }
                    if let note = model.ffmpegNote {
                        Text(note)
                            .font(.caption)
                            .foregroundStyle(.orange)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Text("This step is optional. Skip it now and check again from Setup and Tutorial in the menu bar.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }
}

private struct PracticePage: View {
    @ObservedObject var model: OnboardingModel
    @AppStorage("finderShortcut") private var finderShortcut = true

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            PageTitle("Try it", "Open a practice folder of sample photos, then do each step in Finder. They tick off here as you go.")
            HStack(spacing: 12) {
                Button(model.practiceFolder == nil ? "Open Practice Folder" : "Show Practice Folder Again") {
                    model.openPracticeFolder()
                }
                .controlSize(.large)
                if let problem = model.practiceProblem {
                    Text(problem)
                        .font(.caption)
                        .foregroundStyle(.red)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            StepRow(done: model.done.contains(.convert), number: 1, title: "Convert",
                    detail: "Start dragging a photo. While it moves, hold Shift: a wheel of formats opens. Drop on PNG.")
            StepRow(done: model.done.contains(.tools), number: 2, title: "Tools",
                    detail: "Drag a photo again, this time holding Command. Try Compress or Resize.")
            StepRow(done: model.done.contains(.shortcut), number: 3, title: "No drag",
                    detail: finderShortcut
                        ? "Click a photo, then press \(FinderShortcut.keys). The first time, macOS asks to let Radlet see Finder's selection: click OK."
                        : "\(FinderShortcut.keys) is turned off. Turn it on under Shortcuts and startup to try this.")
            if model.done.count == Practice.allCases.count {
                Label("That's everything. Radlet is in the menu bar whenever you need it.", systemImage: "checkmark.seal.fill")
                    .foregroundStyle(.green)
            } else {
                Text("Let go in the middle of the wheel to cancel. The practice folder is temporary, and macOS clears it away on its own.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

// MARK: - Pieces

private struct KeyRow: View {
    let key: String
    let title: String
    let detail: String

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text(key)
                .font(.system(size: 12, weight: .semibold, design: .rounded))
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .frame(width: 104, alignment: .center)
                .background(RoundedRectangle(cornerRadius: 6).fill(Color.primary.opacity(0.07)))
                .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.primary.opacity(0.15)))
            VStack(alignment: .leading, spacing: 1) {
                Text(title).font(.system(size: 13, weight: .semibold))
                Text(detail).font(.system(size: 12)).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .accessibilityElement(children: .combine)
    }
}

private struct ToggleRow: View {
    let isOn: Binding<Bool>
    let title: String
    let detail: String

    var body: some View {
        Toggle(isOn: isOn) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            // Full width, so every switch lines up at the right edge.
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .toggleStyle(.switch)
    }
}

/// One step of the practice checklist; ticks itself off.
private struct StepRow: View {
    let done: Bool
    let number: Int
    let title: String
    let detail: String

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            ZStack {
                Circle()
                    .fill(done ? Color.green : Color.primary.opacity(0.08))
                    .frame(width: 26, height: 26)
                if done {
                    Image(systemName: "checkmark")
                        .font(.system(size: 12, weight: .bold))
                        .foregroundStyle(.white)
                } else {
                    Text("\(number)").font(.system(size: 12, weight: .semibold))
                }
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.headline)
                Text(detail)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .animation(.spring(response: 0.35, dampingFraction: 0.7), value: done)
        .accessibilityElement(children: .combine)
        .accessibilityValue(done ? "Done" : "Not done yet")
    }
}

// MARK: - Practice folder

/// A temporary folder of sample photos to practise on, drawn here so
/// nothing personal is involved and nothing ships with the app.
enum PracticeFolder {
    private struct Sample {
        let name: String
        let skyTop: CGColor
        let skyBottom: CGColor
        let sun: CGColor
        let land: CGColor
    }

    private static var samples: [Sample] { [
        Sample(name: "Sunset", skyTop: CGColor(red: 0.25, green: 0.18, blue: 0.45, alpha: 1),
              skyBottom: CGColor(red: 0.98, green: 0.55, blue: 0.35, alpha: 1),
              sun: CGColor(red: 1.0, green: 0.86, blue: 0.55, alpha: 1),
              land: CGColor(red: 0.16, green: 0.12, blue: 0.24, alpha: 1)),
        Sample(name: "Lake", skyTop: CGColor(red: 0.20, green: 0.45, blue: 0.80, alpha: 1),
              skyBottom: CGColor(red: 0.70, green: 0.86, blue: 0.97, alpha: 1),
              sun: CGColor(red: 1.0, green: 0.97, blue: 0.85, alpha: 1),
              land: CGColor(red: 0.18, green: 0.42, blue: 0.30, alpha: 1)),
        Sample(name: "Dunes", skyTop: CGColor(red: 0.45, green: 0.66, blue: 0.86, alpha: 1),
              skyBottom: CGColor(red: 0.96, green: 0.86, blue: 0.70, alpha: 1),
              sun: CGColor(red: 1.0, green: 0.95, blue: 0.80, alpha: 1),
              land: CGColor(red: 0.85, green: 0.58, blue: 0.33, alpha: 1)),
    ] }

    enum Problem: LocalizedError {
        case drawing
        var errorDescription: String? { "the sample photos couldn't be drawn" }
    }

    /// Makes the folder if needed, draws any sample photo that's missing
    /// (what the person made there stays), and returns it with its first
    /// photo.
    static func make() throws -> (folder: URL, first: URL) {
        let fm = FileManager.default
        let folder = fm.temporaryDirectory.appendingPathComponent("Radlet Practice", isDirectory: true)
        try fm.createDirectory(at: folder, withIntermediateDirectories: true)
        var photos: [URL] = []
        for sample in samples {
            let url = folder.appendingPathComponent("\(sample.name).jpg")
            if !fm.fileExists(atPath: url.path) { try draw(sample, to: url) }
            photos.append(url)
        }
        return (folder, photos[0])
    }

    private static func draw(_ scene: Sample, to url: URL) throws {
        let width = 1600, height = 1067
        let w = Double(width), h = Double(height)
        guard let space = CGColorSpace(name: CGColorSpace.sRGB),
              let ctx = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
                                  space: space, bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue),
              let sky = CGGradient(colorsSpace: space, colors: [scene.skyBottom, scene.skyTop] as CFArray, locations: nil)
        else { throw Problem.drawing }
        // y runs up: the sky's bottom colour at the horizon, its top above.
        ctx.drawLinearGradient(sky, start: CGPoint(x: 0, y: h * 0.3), end: CGPoint(x: 0, y: h), options: [.drawsBeforeStartLocation])
        ctx.setFillColor(scene.sun)
        ctx.fillEllipse(in: CGRect(x: w * 0.62, y: h * 0.42, width: 210, height: 210))
        ctx.setFillColor(scene.land)
        ctx.move(to: CGPoint(x: 0, y: h * 0.38))
        ctx.addCurve(to: CGPoint(x: w, y: h * 0.33),
                     control1: CGPoint(x: w * 0.3, y: h * 0.52), control2: CGPoint(x: w * 0.65, y: h * 0.18))
        ctx.addLine(to: CGPoint(x: w, y: 0))
        ctx.addLine(to: CGPoint(x: 0, y: 0))
        ctx.closePath()
        ctx.fillPath()
        guard let image = ctx.makeImage(),
              let dest = CGImageDestinationCreateWithURL(url as CFURL, UTType.jpeg.identifier as CFString, 1, nil)
        else { throw Problem.drawing }
        CGImageDestinationAddImage(dest, image, [kCGImageDestinationLossyCompressionQuality: 0.85] as CFDictionary)
        guard CGImageDestinationFinalize(dest) else { throw Problem.drawing }
    }
}
