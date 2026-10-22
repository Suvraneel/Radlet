import AppKit
import VeerKit

/// Notices when the user is dragging files anywhere on screen and presses
/// a trigger key.
///
/// Needs no Accessibility or Input Monitoring permission: it listens only
/// to global mouse events and polls the current modifier flags, mouse
/// buttons and the system drag pasteboard. It never sees keystrokes.
///
/// Every decision it makes is written to the diagnostics log (state
/// changes only), so a gesture that misbehaves on a tester's Mac can be
/// explained from one copied log.
@MainActor
final class DragWatcher {
    var onTrigger: ((WheelMode, [URL], NSPoint) -> Void)?
    var onModeChange: ((WheelMode) -> Void)?
    var onDragEnded: (() -> Void)?
    /// Bundle IDs of apps where the gesture is turned off.
    var excludedApps: () -> Set<String> = { [] }
    var isEnabled: () -> Bool = { true }

    private var monitors: [Any] = []
    private var timer: Timer?
    private let dragPasteboard = NSPasteboard(name: .drag)
    private var baselineChangeCount = 0
    private var cachedChangeCount = -1
    private var cachedURLs: [URL] = []
    private var activeMode: WheelMode?
    private var dragStarted = Date()
    private var reportedNoFiles = false
    private var lastFlags: NSEvent.ModifierFlags = []

    func start() {
        if let m = NSEvent.addGlobalMonitorForEvents(matching: .leftMouseDown, handler: { [weak self] _ in
            MainActor.assumeIsolated { self?.mouseDown() }
        }) {
            monitors.append(m)
        }
        if let m = NSEvent.addGlobalMonitorForEvents(matching: .leftMouseDragged, handler: { [weak self] _ in
            MainActor.assumeIsolated { self?.mouseDragged() }
        }) {
            monitors.append(m)
        }
        Log.drag("watching for drags (monitors installed: \(monitors.count) of 2)")
    }

    func stop() {
        monitors.forEach { NSEvent.removeMonitor($0) }
        monitors.removeAll()
        stopPolling()
    }

    private func mouseDown() {
        baselineChangeCount = dragPasteboard.changeCount
        activeMode = nil
    }

    private func mouseDragged() {
        guard timer == nil, isEnabled() else { return }
        // A file drag replaces the drag pasteboard's contents.
        guard dragPasteboard.changeCount != baselineChangeCount else { return }
        if let app = NSWorkspace.shared.frontmostApplication?.bundleIdentifier, excludedApps().contains(app) {
            Log.drag("drag ignored: \(app) is in the excluded apps")
            baselineChangeCount = dragPasteboard.changeCount
            return
        }
        dragStarted = Date()
        reportedNoFiles = false
        lastFlags = []
        Log.drag("drag started, pasteboard types: \(typeSummary())")
        let t = Timer(timeInterval: 1.0 / 60.0, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.tick() }
        }
        RunLoop.main.add(t, forMode: .common)
        timer = t
    }

    private func tick() {
        if NSEvent.pressedMouseButtons & 1 == 0 {
            Log.drag("drag ended after \(elapsed()) (wheel \(activeMode == nil ? "not shown" : "shown"))")
            stopPolling()
            onDragEnded?()
            return
        }
        let flags = NSEvent.modifierFlags.intersection([.shift, .command, .control, .option])
        if flags != lastFlags {
            Log.drag("modifier keys now: \(describe(flags))")
            lastFlags = flags
        }
        guard let mode = WheelMode.from(flags) else { return }
        if let current = activeMode {
            if mode != current {
                Log.drag("mode changed to \(mode.title)")
                activeMode = mode
                onModeChange?(mode)
            }
            return
        }
        let urls = draggedFileURLs()
        // Finder can publish the file list a moment after the drag starts.
        guard !urls.isEmpty else {
            if !reportedNoFiles, Date().timeIntervalSince(dragStarted) > 1 {
                reportedNoFiles = true
                Log.drag("trigger key held but no file URLs on the drag pasteboard yet (types: \(typeSummary()))")
            }
            return
        }
        activeMode = mode
        Log.drag("trigger: \(mode.title) for \(describeFiles(urls)) after \(elapsed())")
        onTrigger?(mode, urls, NSEvent.mouseLocation)
    }

    private func draggedFileURLs() -> [URL] {
        if dragPasteboard.changeCount == cachedChangeCount, !cachedURLs.isEmpty { return cachedURLs }
        let urls = dragPasteboard.readObjects(forClasses: [NSURL.self],
                                              options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
        // Only cache a real list, so a not-yet-published one is read again.
        if !urls.isEmpty {
            cachedChangeCount = dragPasteboard.changeCount
            cachedURLs = urls
        }
        return urls
    }

    private func stopPolling() {
        timer?.invalidate()
        timer = nil
        activeMode = nil
    }

    private func elapsed() -> String {
        String(format: "%.2f s", Date().timeIntervalSince(dragStarted))
    }

    /// Pasteboard type identifiers only; never the contents.
    private func typeSummary() -> String {
        let types = dragPasteboard.types?.map(\.rawValue) ?? []
        if types.isEmpty { return "none" }
        let shown = types.prefix(6).joined(separator: ", ")
        return types.count > 6 ? "\(shown), +\(types.count - 6) more" : shown
    }

    private func describe(_ flags: NSEvent.ModifierFlags) -> String {
        var names: [String] = []
        if flags.contains(.shift) { names.append("Shift") }
        if flags.contains(.command) { names.append("Command") }
        if flags.contains(.control) { names.append("Control") }
        if flags.contains(.option) { names.append("Option") }
        return names.isEmpty ? "none" : names.joined(separator: "+")
    }
}
