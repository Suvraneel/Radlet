import AppKit
import VeerKit

/// Finder right-click entry points: Services ("Convert with Veer",
/// "Veer Tools") and Open With. They open the wheel for the chosen files
/// in keyboard mode, so conversions work even without the drag gesture.
///
/// The menu items are declared under `NSServices` in Info.plist; each
/// `NSMessage` there names one of the methods below.
final class ServiceProvider: NSObject {
    @objc func convertFiles(_ pboard: NSPasteboard, userData: String?,
                            error: AutoreleasingUnsafeMutablePointer<NSString>) {
        open(pboard, mode: .convert, error: error)
    }

    @objc func toolsForFiles(_ pboard: NSPasteboard, userData: String?,
                             error: AutoreleasingUnsafeMutablePointer<NSString>) {
        open(pboard, mode: .tools, error: error)
    }

    private func open(_ pboard: NSPasteboard, mode: WheelMode, error: AutoreleasingUnsafeMutablePointer<NSString>) {
        let urls = pboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
        guard !urls.isEmpty else {
            error.pointee = "Veer didn't receive any files." as NSString
            Log.app("service \(mode.title): no files on the pasteboard")
            return
        }
        Log.app("service \(mode.title): \(describeFiles(urls))")
        MainActor.assumeIsolated {
            AppState.shared.showWheel(for: urls, mode: mode)
        }
    }
}
