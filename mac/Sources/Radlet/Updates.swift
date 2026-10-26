import AppKit
import Combine
import Sparkle

/// Automatic updates through Sparkle (MIT). Each release on GitHub carries an
/// `appcast.xml` describing it, and the feed address in Info.plist always
/// points at the latest one. Updates are verified with the EdDSA public key
/// in Info.plist before anything is installed.
///
/// A build without a public key (or one run outside an app bundle, like
/// `swift run`) has no updater: Sparkle couldn't verify what it downloads.
@MainActor
final class Updates: NSObject, ObservableObject {
    static let shared = Updates()

    @Published private(set) var canCheck = false
    @Published private(set) var isAvailable = false

    private var controller: SPUStandardUpdaterController?

    /// This copy can update itself: it's an app bundle with a public key.
    nonisolated static var isConfigured: Bool {
        let key = Bundle.main.object(forInfoDictionaryKey: "SUPublicEDKey") as? String ?? ""
        let feed = Bundle.main.object(forInfoDictionaryKey: "SUFeedURL") as? String ?? ""
        return !key.isEmpty && !feed.isEmpty && Bundle.main.bundleURL.pathExtension == "app"
    }

    func start() {
        guard controller == nil, Self.isConfigured else { return }
        let controller = SPUStandardUpdaterController(startingUpdater: true, updaterDelegate: nil, userDriverDelegate: self)
        self.controller = controller
        isAvailable = true
        // Sparkle's own pattern: the menu item is off while a check runs.
        controller.updater.publisher(for: \.canCheckForUpdates)
            .receive(on: RunLoop.main)
            .assign(to: &$canCheck)
    }

    func checkForUpdates() {
        controller?.checkForUpdates(nil)
    }

    /// Bound to the switch in Settings. Sparkle keeps it in user defaults.
    var checksAutomatically: Bool {
        get { controller?.updater.automaticallyChecksForUpdates ?? false }
        set {
            objectWillChange.send()
            controller?.updater.automaticallyChecksForUpdates = newValue
        }
    }
}

extension Updates: SPUStandardUserDriverDelegate {
    /// Radlet is a menu bar app with no Dock icon, so Sparkle asks how to
    /// show an update it found on its own. The answer: bring Radlet forward
    /// so the update window doesn't open behind other apps.
    nonisolated var supportsGentleScheduledUpdateReminders: Bool { true }

    nonisolated func standardUserDriverWillHandleShowingUpdate(
        _ handleShowingUpdate: Bool,
        forUpdate update: SUAppcastItem,
        state: SPUUserUpdateState
    ) {
        guard handleShowingUpdate else { return }
        // Sparkle calls its user driver's delegate on the main thread.
        MainActor.assumeIsolated { NSApp.activate() }
    }
}
