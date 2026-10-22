import AppKit
import SwiftUI
import VeerKit

struct HUDView: View {
    @ObservedObject var progress: JobProgress
    var onCancel: () -> Void
    var onUndo: () -> Void
    var onClose: () -> Void
    var onDragging: (Bool) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 10) {
                icon.frame(width: 18)
                VStack(alignment: .leading, spacing: 1) {
                    Text(progress.title).font(.system(size: 12, weight: .semibold)).lineLimit(1)
                    Text(progress.detail).font(.system(size: 11)).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer(minLength: 8)
                if progress.phase == .running {
                    Button("Cancel", action: onCancel).controlSize(.small)
                } else {
                    Button(action: onClose) { Image(systemName: "xmark") }
                        .buttonStyle(.plain)
                        .foregroundStyle(.secondary)
                        .help("Close")
                }
            }
            if progress.phase == .running {
                ProgressView(value: progress.fraction).progressViewStyle(.linear)
            } else {
                ForEach(Array((progress.failures + progress.skipped).prefix(3).enumerated()), id: \.offset) { _, line in
                    Text(line).font(.system(size: 11)).foregroundStyle(.secondary).lineLimit(2)
                }
                if !progress.outputs.isEmpty {
                    HStack(spacing: 8) {
                        FileDragSource(urls: progress.outputs, onDragging: onDragging)
                            .frame(width: 30, height: 30)
                            .help(progress.outputs.count == 1
                                  ? "Drag into Finder, Mail or any app to copy it there"
                                  : "Drag all \(progress.outputs.count) into Finder, Mail or any app to copy them there")
                        Button("Open") { NSWorkspace.shared.open(progress.outputs.first!) }
                        Button("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting(progress.outputs) }
                        Button("Undo", action: onUndo)
                    }
                    .controlSize(.small)
                }
            }
        }
        .padding(14)
        .frame(width: 340, alignment: .leading)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color.primary.opacity(0.08)))
        .onHover { progress.hovering = $0 }
        .padding(10)
    }

    @ViewBuilder private var icon: some View {
        switch progress.phase {
        case .running: ProgressView().controlSize(.small)
        case .finished: Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
        case .failed: Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.yellow)
        }
    }
}

/// Floating, non-activating panel near the top of the screen.
@MainActor
final class HUDController {
    let progress = JobProgress()
    var onCancel: (() -> Void)?
    var onUndo: (() -> Void)?
    /// Results are being dragged out of the HUD.
    var onDragging: ((Bool) -> Void)?

    private var panel: NSPanel?
    private var hideTask: Task<Void, Never>?
    private var dragging = false

    private func makePanel() -> NSPanel {
        let panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 360, height: 140),
                            styleMask: [.borderless, .nonactivatingPanel],
                            backing: .buffered, defer: false)
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.level = .statusBar
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient]
        let host = NSHostingView(rootView: HUDView(
            progress: progress,
            onCancel: { [weak self] in self?.onCancel?() },
            onUndo: { [weak self] in self?.onUndo?() },
            onClose: { [weak self] in self?.hide() },
            onDragging: { [weak self] dragging in
                // Stay up while dragging; the drag needs its source window.
                self?.dragging = dragging
                self?.onDragging?(dragging)
                if !dragging, self?.progress.phase == .finished { self?.scheduleHide(after: 2) }
            }))
        host.sizingOptions = [.intrinsicContentSize]
        panel.contentView = host
        return panel
    }

    func show() {
        hideTask?.cancel()
        let panel = self.panel ?? makePanel()
        self.panel = panel
        reposition()
        panel.orderFrontRegardless()
    }

    func reposition() {
        guard let panel, let screen = NSScreen.main else { return }
        let size = panel.contentView?.fittingSize ?? NSSize(width: 360, height: 100)
        let f = screen.visibleFrame
        panel.setFrame(NSRect(x: f.midX - size.width / 2, y: f.maxY - size.height - 4,
                              width: size.width, height: size.height), display: true)
    }

    /// Successes fade after a few seconds (not while hovered); problems
    /// stay until closed so the message can be read.
    func finish() {
        reposition()
        guard progress.phase == .finished else { return }
        scheduleHide(after: 4)
    }

    private func scheduleHide(after seconds: Double) {
        hideTask?.cancel()
        hideTask = Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
            guard let self, !Task.isCancelled else { return }
            if self.progress.hovering || self.dragging {
                self.scheduleHide(after: 2)
            } else {
                self.hide()
            }
        }
    }

    func hide() {
        hideTask?.cancel()
        panel?.orderOut(nil)
    }
}

/// The results as a file icon to drag into Finder, Mail or any app.
/// Dragging copies; the files stay where Veer saved them.
struct FileDragSource: NSViewRepresentable {
    let urls: [URL]
    var onDragging: (Bool) -> Void

    func makeNSView(context: Context) -> FileDragView {
        let view = FileDragView()
        view.urls = urls
        view.onDragging = onDragging
        return view
    }

    func updateNSView(_ view: FileDragView, context: Context) {
        view.urls = urls
        view.onDragging = onDragging
    }
}

final class FileDragView: NSView, NSDraggingSource {
    var urls: [URL] = [] {
        didSet { if urls != oldValue { needsDisplay = true } }
    }
    var onDragging: (Bool) -> Void = { _ in }
    private var pressedAt: NSPoint?

    override var intrinsicContentSize: NSSize { NSSize(width: 30, height: 30) }

    // The HUD never becomes active, so the first click must count.
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func mouseDown(with event: NSEvent) {
        pressedAt = convert(event.locationInWindow, from: nil)
    }

    override func mouseUp(with event: NSEvent) {
        pressedAt = nil
    }

    override func mouseDragged(with event: NSEvent) {
        guard let start = pressedAt, !urls.isEmpty else { return }
        let p = convert(event.locationInWindow, from: nil)
        guard hypot(p.x - start.x, p.y - start.y) > 3 else { return }
        pressedAt = nil
        let items: [NSDraggingItem] = urls.enumerated().map { i, url in
            let item = NSDraggingItem(pasteboardWriter: url as NSURL)
            let offset = CGFloat(min(i, 4)) * 4
            item.setDraggingFrame(NSRect(x: offset, y: -offset, width: 30, height: 30), contents: Self.icon(url))
            return item
        }
        let session = beginDraggingSession(with: items, event: event, source: self)
        session.animatesToStartingPositionsOnCancelOrFail = true
    }

    func draggingSession(_ session: NSDraggingSession,
                         sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        .copy
    }

    func draggingSession(_ session: NSDraggingSession, willBeginAt screenPoint: NSPoint) {
        onDragging(true)
    }

    func draggingSession(_ session: NSDraggingSession, endedAt screenPoint: NSPoint, operation: NSDragOperation) {
        onDragging(false)
    }

    override func draw(_ dirtyRect: NSRect) {
        // Up to three icons, fanned out like a stack of papers.
        for (i, url) in Array(urls.prefix(3).enumerated()).reversed() {
            let o = CGFloat(i) * 2
            Self.icon(url).draw(in: NSRect(x: o, y: o, width: 26, height: 26))
        }
    }

    private static func icon(_ url: URL) -> NSImage {
        let image = NSWorkspace.shared.icon(forFile: url.path)
        image.size = NSSize(width: 30, height: 30)
        return image
    }
}
