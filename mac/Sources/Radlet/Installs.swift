import AppKit
import CryptoKit
import SwiftUI
import RadletKit

/// Installs FFmpeg for Radlet: with Homebrew when it's there, otherwise
/// Radlet's own build (pinned in the app by URL and SHA-256), unpacked into
/// ~/Library/Application Support/Radlet/FFmpeg, where the engine looks.
@MainActor
final class FFmpegInstaller: ObservableObject {
    enum Phase: Equatable { case idle, working, finished, failed }

    @Published private(set) var phase: Phase = .idle
    @Published private(set) var status = ""
    /// nil while there's no way to tell how far along it is.
    @Published private(set) var fraction: Double?

    private var process: Process?
    private var download: URLSessionDownloadTask?
    private var watch: NSKeyValueObservation?
    private var cancelled = false

    /// Homebrew's `brew`, on Apple silicon or Intel.
    static var homebrew: URL? {
        ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]
            .map { URL(fileURLWithPath: $0) }
            .first { FileManager.default.isExecutableFile(atPath: $0.path) }
    }

    /// Radlet's build, once one has been published and pinned.
    static var pinned: PinnedFFmpeg? {
        guard let url = Bundle.main.url(forResource: "ffmpeg-build", withExtension: "json"),
              let data = try? Data(contentsOf: url) else { return nil }
        return PinnedFFmpeg.parse(data)
    }

    static var canInstall: Bool { homebrew != nil || pinned != nil }

    /// Where the engine finds an FFmpeg the app installed.
    static var folder: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Radlet/FFmpeg", isDirectory: true)
    }

    /// How it'll be installed, for the window.
    var plan: String {
        if Self.homebrew != nil { return "Radlet installs it with Homebrew, which can take a few minutes." }
        if let pin = Self.pinned {
            return "Radlet downloads FFmpeg \(pin.version) (\(pin.sizeText)), checks it and keeps it in its own folder."
        }
        return "Install it with Homebrew: paste brew install ffmpeg into Terminal, then choose this again."
    }

    /// Installs FFmpeg and has the engine find it. True when it's ready;
    /// false when it failed or was cancelled.
    func install() async -> Bool {
        guard phase != .working else { return false }
        phase = .working
        cancelled = false
        fraction = nil
        do {
            if let brew = Self.homebrew {
                do {
                    try await installWithHomebrew(brew)
                } catch where !cancelled && Self.pinned != nil {
                    Log.app("Homebrew couldn't install FFmpeg (\(error.localizedDescription)); trying Radlet's build")
                    try await installBuild(Self.pinned!)
                }
            } else if let pin = Self.pinned {
                try await installBuild(pin)
            } else {
                throw InstallError("There's no Homebrew here, and this copy of Radlet has no FFmpeg to download.")
            }
            try checkCancelled()
            status = "Checking…"
            fraction = nil
            _ = try await EngineClient.shared.refresh()
            let found = try await EngineClient.shared.hello().capabilities.ffmpeg
            try checkCancelled()
            guard found else {
                throw InstallError("FFmpeg was installed, but Radlet can't find it. Try restarting Radlet.")
            }
            status = "FFmpeg is ready."
            phase = .finished
            Log.app("FFmpeg installed")
            await AppState.shared.checkEngine()
            return true
        } catch {
            if cancelled {
                status = "Cancelled."
                phase = .idle
            } else {
                status = error.localizedDescription
                phase = .failed
                Log.app("FFmpeg install failed: \(error.localizedDescription)")
            }
            return false
        }
    }

    func cancel() {
        guard phase == .working, !cancelled else { return }
        cancelled = true
        download?.cancel()
        // Homebrew tidies up after an interrupt (Control-C); if it hasn't
        // stopped after a few seconds, it's stopped outright.
        if let process, process.isRunning {
            process.interrupt()
            DispatchQueue.main.asyncAfter(deadline: .now() + 5) {
                if process.isRunning { process.terminate() }
            }
        }
    }

    /// Stops after a step that finished just as the person cancelled.
    private func checkCancelled() throws {
        if cancelled { throw CancellationError() }
    }

    /// True on Apple silicon, even when Radlet runs under Rosetta.
    private static var appleSilicon: Bool {
        var value: Int32 = 0
        var size = MemoryLayout<Int32>.size
        return sysctlbyname("hw.optional.arm64", &value, &size, nil, 0) == 0 && value == 1
    }

    // MARK: Homebrew

    private func installWithHomebrew(_ brew: URL) async throws {
        status = "Installing with Homebrew…"
        let p = Process()
        if Self.appleSilicon && brew.path.hasPrefix("/usr/local/") {
            // An Intel Homebrew on Apple silicon only runs through Rosetta.
            p.executableURL = URL(fileURLWithPath: "/usr/bin/arch")
            p.arguments = ["-x86_64", brew.path, "install", "ffmpeg"]
        } else {
            p.executableURL = brew
            p.arguments = ["install", "ffmpeg"]
        }
        var env = ProcessInfo.processInfo.environment
        env["NONINTERACTIVE"] = "1"
        env["HOMEBREW_NO_ENV_HINTS"] = "1"
        env["HOMEBREW_NO_EMOJI"] = "1"
        // Just FFmpeg: no updating every formula first, and no rebuilding
        // the other things installed with Homebrew.
        env["HOMEBREW_NO_AUTO_UPDATE"] = "1"
        env["HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK"] = "1"
        env["PATH"] = brew.deletingLastPathComponent().path + ":/usr/bin:/bin:/usr/sbin:/sbin"
        p.environment = env
        let pipe = Pipe()
        p.standardOutput = pipe
        p.standardError = pipe
        p.standardInput = FileHandle.nullDevice
        let tail = OutputTail()
        pipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            guard !data.isEmpty, let line = tail.add(data) else { return }
            DispatchQueue.main.async { MainActor.assumeIsolated { self?.status = line } }
        }
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            p.terminationHandler = { _ in cont.resume() }
            do {
                try p.run()
                process = p
            } catch {
                p.terminationHandler = nil
                cont.resume(throwing: error)
            }
        }
        pipe.fileHandleForReading.readabilityHandler = nil
        process = nil
        try checkCancelled()
        guard p.terminationStatus == 0 else {
            throw InstallError("Homebrew couldn't install FFmpeg (\(tail.last ?? "exit \(p.terminationStatus)")).")
        }
    }

    // MARK: Radlet's build

    private func installBuild(_ pin: PinnedFFmpeg) async throws {
        guard let url = URL(string: pin.url) else { throw InstallError("The FFmpeg download address is wrong.") }
        let fm = FileManager.default
        let parent = Self.folder.deletingLastPathComponent()
        try fm.createDirectory(at: parent, withIntermediateDirectories: true)
        Self.sweep(parent)
        status = "Downloading FFmpeg \(pin.version)…"
        fraction = 0
        let zip = try await fetch(url, limit: pin.size)
        defer { try? fm.removeItem(at: zip) }
        try checkCancelled()
        status = "Checking the download…"
        fraction = nil
        let size = (try? fm.attributesOfItem(atPath: zip.path)[.size] as? NSNumber)?.int64Value
        guard size == pin.size else {
            throw InstallError("The download wasn't the size Radlet expects, so it wasn't used.")
        }
        let sum = try await Task.detached { try Self.sha256(of: zip) }.value
        try checkCancelled()
        guard sum == pin.sha256.lowercased() else {
            throw InstallError("The download didn't match what Radlet expects, so it wasn't used.")
        }
        let staging = parent.appendingPathComponent("FFmpeg.partial-\(UUID().uuidString)", isDirectory: true)
        defer { try? fm.removeItem(at: staging) }
        try fm.createDirectory(at: staging, withIntermediateDirectories: true)
        try await Self.run("/usr/bin/ditto", ["-x", "-k", zip.path, staging.path])
        for tool in ["ffmpeg", "ffprobe"] {
            let path = staging.appendingPathComponent(tool).path
            guard fm.isExecutableFile(atPath: path) else { throw InstallError("The download has no \(tool).") }
        }
        try await Self.run(staging.appendingPathComponent("ffmpeg").path, ["-hide_banner", "-version"])
        try checkCancelled()
        // Swap it in only once it's known to work, keeping the earlier copy
        // until the new one is in place.
        let old = parent.appendingPathComponent("FFmpeg.old-\(UUID().uuidString)", isDirectory: true)
        let hadOne = fm.fileExists(atPath: Self.folder.path)
        if hadOne { try fm.moveItem(at: Self.folder, to: old) }
        do {
            try fm.moveItem(at: staging, to: Self.folder)
        } catch {
            if hadOne { try? fm.moveItem(at: old, to: Self.folder) }
            throw error
        }
        if hadOne { try? fm.removeItem(at: old) }
    }

    /// Clears what an interrupted install left beside the FFmpeg folder,
    /// putting back an earlier copy that was moved aside but not replaced.
    private static func sweep(_ parent: URL) {
        let fm = FileManager.default
        guard let names = try? fm.contentsOfDirectory(atPath: parent.path) else { return }
        for name in names where name.hasPrefix("FFmpeg.partial-") || name.hasPrefix("FFmpeg.old-") {
            let item = parent.appendingPathComponent(name, isDirectory: true)
            if name.hasPrefix("FFmpeg.old-") && !fm.fileExists(atPath: folder.path),
               (try? fm.moveItem(at: item, to: folder)) != nil {
                continue
            }
            try? fm.removeItem(at: item)
        }
    }

    /// Downloads to a temporary file, reporting progress, and gives up on
    /// anything bigger than `limit` bytes.
    private func fetch(_ url: URL, limit: Int64) async throws -> URL {
        let oversize = Flag()
        defer {
            watch = nil
            download = nil
        }
        return try await withCheckedThrowingContinuation { (cont: CheckedContinuation<URL, Error>) in
            let task = URLSession.shared.downloadTask(with: url) { temporary, response, error in
                if oversize.isSet {
                    if let temporary { try? FileManager.default.removeItem(at: temporary) }
                    cont.resume(throwing: InstallError("The download was bigger than Radlet expects, so it stopped."))
                    return
                }
                if let error {
                    cont.resume(throwing: error)
                    return
                }
                guard let temporary, let http = response as? HTTPURLResponse, http.statusCode == 200 else {
                    let code = (response as? HTTPURLResponse)?.statusCode ?? 0
                    cont.resume(throwing: InstallError("The download failed (HTTP \(code))."))
                    return
                }
                // The temporary file goes when this returns, so keep it.
                let kept = FileManager.default.temporaryDirectory
                    .appendingPathComponent("radlet-ffmpeg-\(UUID().uuidString).zip")
                do {
                    try FileManager.default.moveItem(at: temporary, to: kept)
                    cont.resume(returning: kept)
                } catch {
                    cont.resume(throwing: error)
                }
            }
            watch = task.progress.observe(\.fractionCompleted) { [weak self, weak task] progress, _ in
                if progress.completedUnitCount > limit || task.map({ $0.countOfBytesExpectedToReceive > limit }) == true {
                    oversize.set()
                    task?.cancel()
                    return
                }
                let done = progress.fractionCompleted
                DispatchQueue.main.async { MainActor.assumeIsolated { self?.fraction = done } }
            }
            download = task
            task.resume()
        }
    }

    nonisolated static func sha256(of url: URL) throws -> String {
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        var hasher = SHA256()
        while let chunk = try handle.read(upToCount: 1 << 20), !chunk.isEmpty {
            hasher.update(data: chunk)
        }
        return hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }

    /// Runs a program off the main thread and waits for it to succeed.
    private static func run(_ path: String, _ arguments: [String]) async throws {
        try await Task.detached { try FFmpegInstaller.runTool(path, arguments) }.value
    }

    nonisolated private static func runTool(_ path: String, _ arguments: [String]) throws {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: path)
        p.arguments = arguments
        p.standardOutput = FileHandle.nullDevice
        p.standardError = FileHandle.nullDevice
        try p.run()
        p.waitUntilExit()
        guard p.terminationStatus == 0 else {
            throw InstallError("\(URL(fileURLWithPath: path).lastPathComponent) stopped (exit \(p.terminationStatus)).")
        }
    }
}

/// A yes/no set on one thread and read on another.
private final class Flag: @unchecked Sendable {
    private let lock = NSLock()
    private var value = false
    var isSet: Bool {
        lock.lock()
        defer { lock.unlock() }
        return value
    }
    func set() {
        lock.lock()
        value = true
        lock.unlock()
    }
}

struct InstallError: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}

/// Keeps the last meaningful line of a program's output.
private final class OutputTail: @unchecked Sendable {
    private let lock = NSLock()
    private var partial = ""
    private(set) var last: String?

    /// Adds output; returns the newest complete line worth showing.
    func add(_ data: Data) -> String? {
        lock.lock()
        defer { lock.unlock() }
        partial += String(decoding: data, as: UTF8.self)
        var lines = partial.components(separatedBy: .newlines)
        partial = lines.removeLast()
        let shown = lines
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
            .map { $0.hasPrefix("==> ") ? String($0.dropFirst(4)) : $0 }
        if let line = shown.last { last = line }
        return shown.last
    }
}

// MARK: - The window

/// What the window shows for one need.
@MainActor
final class InstallModel: ObservableObject {
    let need: Need
    let task: String?
    let installer = FFmpegInstaller()
    /// What to carry on with once it's installed: every job that asked
    /// while this window was open.
    private var readies: [() -> Void]
    /// Closes this model's window (set by the controller).
    var close: () -> Void = {}

    init(need: Need, task: String?, ready: @escaping () -> Void) {
        self.need = need
        self.task = task
        readies = [ready]
    }

    /// Another job needs the same thing.
    func also(_ ready: @escaping () -> Void) {
        readies.append(ready)
    }

    var explanation: String { need.explanation(task: task) }

    var canAct: Bool {
        need != .ffmpeg || (FFmpegInstaller.canInstall && installer.phase != .working)
    }

    var actionTitle: String {
        if need == .ffmpeg && installer.phase == .failed { return "Try Again" }
        return need.actionTitle
    }

    func act() {
        if let destination = need.destination {
            NSWorkspace.shared.open(destination)
            close()
            return
        }
        Task {
            guard await installer.install() else { return }
            let waiting = readies
            readies = []
            close()
            for ready in waiting { ready() }
        }
    }

    func dismiss() {
        installer.cancel()
        readies = []
        close()
    }

    func copyCommand() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString("brew install ffmpeg", forType: .string)
    }
}

struct InstallView: View {
    @ObservedObject var model: InstallModel
    @ObservedObject var installer: FFmpegInstaller

    init(model: InstallModel) {
        self.model = model
        installer = model.installer
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(alignment: .top, spacing: 14) {
                Image(systemName: model.need.symbol)
                    .font(.system(size: 28, weight: .light))
                    .foregroundStyle(.tint)
                    .frame(width: 36)
                VStack(alignment: .leading, spacing: 10) {
                    Text(model.need.title).font(.headline)
                    Text(model.explanation)
                        .fixedSize(horizontal: false, vertical: true)
                    if model.need == .ffmpeg {
                        Text(installer.plan)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                        progress
                    }
                }
            }
            .padding(20)
            Divider()
            HStack(spacing: 10) {
                if model.need == .ffmpeg && !FFmpegInstaller.canInstall {
                    Button("Copy Command") { model.copyCommand() }
                    Link("What's Homebrew?", destination: URL(string: "https://brew.sh")!)
                }
                Spacer()
                Button(installer.phase == .working ? "Cancel" : "Not Now") { model.dismiss() }
                    .keyboardShortcut(.cancelAction)
                if model.need != .ffmpeg || FFmpegInstaller.canInstall {
                    Button(model.actionTitle) { model.act() }
                        .keyboardShortcut(.defaultAction)
                        .disabled(!model.canAct)
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
        }
        .frame(width: 480)
    }

    @ViewBuilder private var progress: some View {
        switch installer.phase {
        case .idle:
            if !installer.status.isEmpty {
                Text(installer.status).font(.callout).foregroundStyle(.secondary)
            }
        case .working:
            VStack(alignment: .leading, spacing: 6) {
                if let fraction = installer.fraction {
                    ProgressView(value: fraction)
                } else {
                    ProgressView().progressViewStyle(.linear)
                }
                Text(installer.status)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                    .truncationMode(.middle)
            }
        case .finished:
            Label(installer.status, systemImage: "checkmark.circle.fill").foregroundStyle(.green)
        case .failed:
            Text(installer.status)
                .font(.callout)
                .foregroundStyle(.red)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// One window per need: asking again for the same need brings its window
/// forward and adds to what runs once it's got; a different need gets a
/// window of its own and leaves the others be.
@MainActor
final class InstallWindowController: NSObject, NSWindowDelegate {
    static let shared = InstallWindowController()
    private var open: [Need: (window: NSWindow, model: InstallModel)] = [:]

    /// Explains `need` for `task` and offers to get it; `ready` runs once
    /// FFmpeg is installed (the other needs are got outside Radlet).
    func offer(_ need: Need, task: String?, ready: @escaping () -> Void = {}) {
        if let existing = open[need] {
            existing.model.also(ready)
            NSApp.activate()
            existing.window.makeKeyAndOrderFront(nil)
            return
        }
        Log.app("offering \(need.rawValue) for \(task ?? "a job")")
        let model = InstallModel(need: need, task: task, ready: ready)
        let host = NSHostingController(rootView: InstallView(model: model))
        let w = NSWindow(contentViewController: host)
        w.title = "Radlet"
        w.styleMask = [.titled, .closable]
        w.isReleasedWhenClosed = false
        w.delegate = self
        model.close = { [weak w] in w?.close() }
        if let last = open.values.map(\.window).last {
            // Offset from one already open, so neither hides the other.
            w.setFrameTopLeftPoint(NSPoint(x: last.frame.minX + 24, y: last.frame.maxY - 24))
        } else {
            w.center()
        }
        open[need] = (w, model)
        NSApp.activate()
        w.makeKeyAndOrderFront(nil)
    }

    func windowWillClose(_ notification: Notification) {
        guard let closing = notification.object as? NSWindow,
              let need = open.first(where: { $0.value.window === closing })?.key,
              let entry = open.removeValue(forKey: need) else { return }
        // Closing the window is "Not Now": an install under way stops.
        entry.model.installer.cancel()
    }
}
