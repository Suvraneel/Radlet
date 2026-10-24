import AppKit
import Carbon.HIToolbox
import RadletKit

/// ⌃⌥V in Finder opens the wheel for the files selected there: the
/// keyboard way in, no drag needed. The shortcut is registered with the
/// system only while Finder is the front app, so other apps keep ⌃⌥V and
/// Radlet never reads other keystrokes. Finder's selection is read with one
/// Apple Event, which macOS asks permission for once.
@MainActor
final class FinderShortcut {
    static let shared = FinderShortcut()

    /// Called with Finder's selection, or a problem to show.
    var onPress: ((Result<[URL], FinderSelectionError>) -> Void)?

    private var hotKey: EventHotKeyRef?
    private var handler: EventHandlerRef?
    private var enabled = false
    private var watcher: NSObjectProtocol?

    /// Shown in Settings and messages.
    static let keys = "⌃⌥V"

    func setEnabled(_ on: Bool) {
        enabled = on
        if watcher == nil {
            watcher = NSWorkspace.shared.notificationCenter.addObserver(
                forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main
            ) { _ in
                MainActor.assumeIsolated { FinderShortcut.shared.update() }
            }
        }
        update()
    }

    /// Registered while it's on and Finder is in front.
    private func update() {
        let finderInFront = NSWorkspace.shared.frontmostApplication?.bundleIdentifier == "com.apple.finder"
        if enabled && finderInFront { register() } else { unregister() }
    }

    private func register() {
        guard hotKey == nil else { return }
        if handler == nil {
            var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
            let status = InstallEventHandler(GetApplicationEventTarget(), { _, _, _ in
                // Hot key events arrive on the main thread.
                DispatchQueue.main.async {
                    MainActor.assumeIsolated { FinderShortcut.shared.pressed() }
                }
                return noErr
            }, 1, &spec, nil, &handler)
            if status != noErr {
                Log.app("shortcut handler failed (\(status))")
                return
            }
        }
        let id = EventHotKeyID(signature: OSType(0x5244_4C54), id: 1)  // "RDLT"
        let status = RegisterEventHotKey(UInt32(kVK_ANSI_V), UInt32(controlKey | optionKey), id,
                                         GetApplicationEventTarget(), 0, &hotKey)
        if status != noErr {
            hotKey = nil
            Log.app("shortcut \(Self.keys) unavailable (\(status)); another app may use it")
        }
    }

    private func unregister() {
        if let hotKey { UnregisterEventHotKey(hotKey) }
        hotKey = nil
    }

    private func pressed() {
        Log.app("shortcut pressed")
        onPress?(FinderSelection.read())
    }
}

enum FinderSelectionError: Error {
    case nothingSelected
    case notAllowed
    case failed(String)

    var message: String {
        switch self {
        case .nothingSelected:
            return "Select files in Finder, then press \(FinderShortcut.keys)"
        case .notAllowed:
            return "Allow Radlet to control Finder in System Settings → Privacy & Security → Automation"
        case .failed(let why):
            return "Couldn't read Finder's selection: \(why)"
        }
    }
}

enum FinderSelection {
    // A busy Finder gets a few seconds, not the usual two minutes.
    private static let source = """
    with timeout of 8 seconds
        tell application "Finder"
            set chosen to selection as alias list
            set paths to {}
            repeat with f in chosen
                set end of paths to POSIX path of f
            end repeat
            return paths
        end tell
    end timeout
    """

    /// The files and folders selected in Finder, in Finder's order.
    static func read() -> Result<[URL], FinderSelectionError> {
        guard let script = NSAppleScript(source: source) else { return .failure(.failed("script")) }
        var error: NSDictionary?
        let result = script.executeAndReturnError(&error)
        if let error {
            let code = error[NSAppleScript.errorNumber] as? Int ?? 0
            Log.app("Finder selection failed (\(code))")
            switch code {
            case -1743: return .failure(.notAllowed)  // not allowed to send Apple Events to Finder
            case -1712: return .failure(.failed("Finder didn't answer in time; try again"))
            default: return .failure(.failed("error \(code)"))
            }
        }
        var urls: [URL] = []
        let count = result.numberOfItems
        if count > 0 {
            for i in 1...count {
                if let path = result.atIndex(i)?.stringValue, !path.isEmpty {
                    urls.append(URL(fileURLWithPath: path))
                }
            }
        }
        return urls.isEmpty ? .failure(.nothingSelected) : .success(urls)
    }
}
