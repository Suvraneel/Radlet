import AppKit
import VeerKit

/// Short names for the diagnostics log categories.
enum Log {
    static func drag(_ message: String) { DiagnosticsLog.shared.log("drag", message) }
    static func wheel(_ message: String) { DiagnosticsLog.shared.log("wheel", message) }
    static func engine(_ message: String) { DiagnosticsLog.shared.log("engine", message) }
    static func app(_ message: String) { DiagnosticsLog.shared.log("app", message) }

    /// An error as its domain and code. File errors' descriptions name the
    /// file, and the log never records names.
    static func code(_ error: Error) -> String {
        let e = error as NSError
        return "\(e.domain) \(e.code)"
    }
}

/// Builds the text testers paste into a bug report.
@MainActor
enum Diagnostics {
    static func report(engineStatus: String) -> String {
        let info = Bundle.main.infoDictionary ?? [:]
        let version = info["CFBundleShortVersionString"] as? String ?? "dev"
        let build = info["CFBundleVersion"] as? String ?? "0"
        let os = ProcessInfo.processInfo.operatingSystemVersionString
        #if arch(arm64)
        let arch = "Apple silicon"
        #else
        let arch = "Intel"
        #endif
        let lines = [
            "Veer diagnostics",
            "App: \(version) (\(build))   macOS: \(os)   Mac: \(arch)",
            "Engine: \(engineStatus)",
            "Wheel gesture: \(Prefs.gestureEnabled ? "on" : "off")   Excluded apps: \(Prefs.excludedApps.count)",
            "Isolation: \(Prefs.isolate ? "on" : "off")   Output: \(Prefs.outputChoice.rawValue)",
            "--- last 200 log lines (no file names) ---",
            DiagnosticsLog.shared.tail(200),
        ]
        return lines.joined(separator: "\n")
    }

    static func copy(engineStatus: String) {
        let text = report(engineStatus: engineStatus)
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    static func revealLog() {
        let log = DiagnosticsLog.shared
        log.flush()
        if FileManager.default.fileExists(atPath: log.fileURL.path) {
            NSWorkspace.shared.activateFileViewerSelecting([log.fileURL])
        } else {
            NSWorkspace.shared.open(log.fileURL.deletingLastPathComponent())
        }
    }
}
