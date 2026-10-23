import AppKit
import VeerKit

/// Share and Copy image, the wheel's quick actions for sending files on.
@MainActor
final class Sharing: NSObject, NSSharingServicePickerDelegate {
    static let shared = Sharing()

    /// A tiny clear window at the pointer that the share menu hangs from.
    private var anchor: NSPanel?

    /// Sends `files` with a named service, or opens the share menu where
    /// the wheel was. Returns a problem to show, if any.
    func share(_ files: [URL], via: QuickAction.ShareVia?) -> String? {
        guard let via else {
            showPicker(files)
            return nil
        }
        let name: NSSharingService.Name
        switch via {
        case .airDrop: name = .sendViaAirDrop
        case .mail: name = .composeEmail
        case .messages: name = .composeMessage
        }
        guard let service = NSSharingService(named: name), service.canPerform(withItems: files) else {
            return "\(via.label) can't send \(files.count == 1 ? "this" : "these") here"
        }
        NSApp.activate()
        service.perform(withItems: files)
        return nil
    }

    private func showPicker(_ files: [URL]) {
        anchor?.orderOut(nil)
        let at = NSEvent.mouseLocation
        let panel = NSPanel(contentRect: NSRect(x: at.x - 1, y: at.y - 1, width: 2, height: 2),
                            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.level = .statusBar
        panel.isReleasedWhenClosed = false
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient]
        let view = NSView(frame: NSRect(x: 0, y: 0, width: 2, height: 2))
        panel.contentView = view
        panel.orderFrontRegardless()
        anchor = panel
        NSApp.activate()
        let picker = NSSharingServicePicker(items: files)
        picker.delegate = self
        picker.show(relativeTo: view.bounds, of: view, preferredEdge: .minY)
    }

    func sharingServicePicker(_ picker: NSSharingServicePicker, didChoose service: NSSharingService?) {
        // The chosen service brings up its own window; the anchor isn't
        // needed any more (kept until the next share, out of sight).
        anchor?.orderOut(nil)
    }

    /// Puts one picture on the clipboard as an image (PNG and TIFF), for
    /// pasting into chats, documents and image editors.
    func copyImage(_ url: URL) -> Bool {
        guard let image = NSImage(contentsOf: url), let tiff = image.tiffRepresentation else { return false }
        let item = NSPasteboardItem()
        if let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:]) {
            item.setData(png, forType: .png)
        }
        item.setData(tiff, forType: .tiff)
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        return pasteboard.writeObjects([item])
    }
}
