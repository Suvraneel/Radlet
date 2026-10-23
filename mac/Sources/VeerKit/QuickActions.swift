import Foundation

/// The app's own quick actions on the tools wheel. They run at once, in
/// the app, with no engine job: what people otherwise reach for in
/// Finder's context menu.
public enum QuickAction: String, CaseIterable, Sendable {
    case copyPath = "app_copy_path"
    case copyName = "app_copy_name"
    case openTerminal = "app_terminal"

    public var slot: Slot {
        let action: JSONValue = .object(["kind": .string(rawValue)])
        switch self {
        case .copyPath:
            return Slot(id: "copy_path", label: "Copy path", symbol: "link", group: "tool", action: action)
        case .copyName:
            return Slot(id: "copy_name", label: "Copy name", symbol: "textformat.abc", group: "tool", action: action)
        case .openTerminal:
            return Slot(id: "open_terminal", label: "Open in Terminal", symbol: "terminal", group: "tool",
                        action: action)
        }
    }

    /// The quick action a slot runs, if it is one.
    public init?(slot: Slot) {
        guard let kind = slot.action["kind"]?.string, let action = QuickAction(rawValue: kind) else { return nil }
        self = action
    }

    /// What goes on the clipboard: one full path, or one name, per line,
    /// in the order the files were chosen.
    public func clipboardText(for files: [URL]) -> String? {
        switch self {
        case .copyPath: return files.isEmpty ? nil : files.map(\.path).joined(separator: "\n")
        case .copyName: return files.isEmpty ? nil : files.map(\.lastPathComponent).joined(separator: "\n")
        case .openTerminal: return nil
        }
    }

    /// The HUD's line once it's done.
    public func doneTitle(count: Int) -> String {
        switch self {
        case .copyPath: return count == 1 ? "Path copied" : "\(count) paths copied"
        case .copyName: return count == 1 ? "Name copied" : "\(count) names copied"
        case .openTerminal: return count == 1 ? "Opened in Terminal" : "Opened \(count) Terminal windows"
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
