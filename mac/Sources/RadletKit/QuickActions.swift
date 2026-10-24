import Foundation

/// The app's own quick actions on the tools wheel. They run at once, in
/// the app, with no engine job: what people otherwise reach for in
/// Finder's context menu.
public enum QuickAction: String, CaseIterable, Sendable {
    case copyPath = "app_copy_path"
    case copyName = "app_copy_name"
    case openTerminal = "app_terminal"
    case share = "app_share"
    case copyImage = "app_copy_image"

    /// Where Share sends the files: a named service, or (nil) the system
    /// share menu at the pointer.
    public enum ShareVia: String, CaseIterable, Sendable {
        case airDrop = "airdrop"
        case mail
        case messages

        public var label: String {
            switch self {
            case .airDrop: return "AirDrop"
            case .mail: return "Mail"
            case .messages: return "Messages"
            }
        }

        public var hint: String {
            switch self {
            case .airDrop: return "Send to a nearby Mac, iPhone or iPad"
            case .mail: return "Attach to a new email"
            case .messages: return "Send in a new message"
            }
        }
    }

    private func action(via: ShareVia? = nil) -> JSONValue {
        var object: [String: JSONValue] = ["kind": .string(rawValue)]
        if let via { object["via"] = .string(via.rawValue) }
        return .object(object)
    }

    public var slot: Slot {
        let action = self.action()
        switch self {
        case .copyPath:
            return Slot(id: "copy_path", label: "Copy path", symbol: "link", group: "tool", action: action)
        case .copyName:
            return Slot(id: "copy_name", label: "Copy name", symbol: "textformat.abc", group: "tool", action: action)
        case .openTerminal:
            return Slot(id: "open_terminal", label: "Open in Terminal", symbol: "terminal", group: "tool",
                        action: action)
        case .share:
            let presets = ShareVia.allCases.map {
                SlotVariant(id: "share_\($0.rawValue)", label: $0.label, hint: $0.hint, action: self.action(via: $0))
            }
            return Slot(id: "share", label: "Share…", symbol: "square.and.arrow.up", group: "tool",
                        action: action, variants: presets)
        case .copyImage:
            return Slot(id: "copy_image", label: "Copy image", symbol: "photo.on.rectangle", group: "tool",
                        action: action)
        }
    }

    /// The quick action a slot runs, if it is one.
    public init?(slot: Slot) {
        guard let kind = slot.action["kind"]?.string, let action = QuickAction(rawValue: kind) else { return nil }
        self = action
    }

    /// The service a Share slot (or one of its presets) names.
    public static func shareVia(_ slot: Slot) -> ShareVia? {
        slot.action["via"]?.string.flatMap(ShareVia.init(rawValue:))
    }

    /// Picture files the clipboard can hold as an image, by extension.
    public static let imageExtensions: Set<String> = [
        "png", "jpg", "jpeg", "heic", "heif", "gif", "tif", "tiff", "bmp", "webp", "avif",
    ]

    /// The quick actions for a selection: all of them, except Copy image,
    /// which is for one picture.
    public static func offered(for files: [URL]) -> [QuickAction] {
        allCases.filter { action in
            switch action {
            case .copyImage:
                return files.count == 1 && imageExtensions.contains(files[0].pathExtension.lowercased())
            case .share:
                return !files.isEmpty
            default:
                return true
            }
        }
    }

    /// What goes on the clipboard: one full path, or one name, per line,
    /// in the order the files were chosen.
    public func clipboardText(for files: [URL]) -> String? {
        switch self {
        case .copyPath: return files.isEmpty ? nil : files.map(\.path).joined(separator: "\n")
        case .copyName: return files.isEmpty ? nil : files.map(\.lastPathComponent).joined(separator: "\n")
        case .openTerminal, .share, .copyImage: return nil
        }
    }

    /// The HUD's line once it's done.
    public func doneTitle(count: Int) -> String {
        switch self {
        case .copyPath: return count == 1 ? "Path copied" : "\(count) paths copied"
        case .copyName: return count == 1 ? "Name copied" : "\(count) names copied"
        case .openTerminal: return count == 1 ? "Opened in Terminal" : "Opened \(count) Terminal windows"
        case .share: return count == 1 ? "Shared" : "Shared \(count) files"
        case .copyImage: return "Image copied"
        }
    }

    /// Folders to open in Terminal: each chosen folder, or the folder a
    /// chosen file is in, once each, in order.
    public static func terminalFolders(for files: [URL], isFolder: (URL) -> Bool) -> [URL] {
        var out: [URL] = []
        for f in files {
            let dir = isFolder(f) ? f : f.deletingLastPathComponent()
            let std = dir.standardizedFileURL
            if !out.contains(where: { $0.standardizedFileURL.path == std.path }) { out.append(dir) }
        }
        return out
    }
}
