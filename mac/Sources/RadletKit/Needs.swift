import Foundation

/// Something a conversion needs that this Mac can get: a program, one of
/// Apple's free apps, or a download in System Settings. The engine marks
/// wheel slots and failed items with these (`needs`, `error.need`); the
/// app explains each and offers to get it.
public enum Need: String, CaseIterable, Sendable {
    case ffmpeg
    case keynote
    case pages
    case numbers
    case speechLanguage = "speech_language"
    case translationLanguage = "translation_language"

    /// The thing itself, for "Needs …".
    public var name: String {
        switch self {
        case .ffmpeg: return "FFmpeg"
        case .keynote: return "Keynote"
        case .pages: return "Pages"
        case .numbers: return "Numbers"
        case .speechLanguage: return "a speech language"
        case .translationLanguage: return "translation languages"
        }
    }

    /// A short note for the wheel: "Needs FFmpeg".
    public var badge: String { "Needs \(name)" }

    /// SF Symbol for the window and the wheel's badge.
    public var symbol: String {
        switch self {
        case .ffmpeg: return "arrow.down.circle"
        case .keynote, .pages, .numbers: return "bag"
        case .speechLanguage: return "waveform"
        case .translationLanguage: return "character.bubble"
        }
    }

    /// The window's heading.
    public var title: String {
        switch self {
        case .ffmpeg: return "Install FFmpeg?"
        case .keynote, .pages, .numbers: return "Get \(name)?"
        case .speechLanguage: return "Download the speech language?"
        case .translationLanguage: return "Download the languages?"
        }
    }

    /// Why, for the task the person chose ("Convert to MP3").
    public func explanation(task: String?) -> String {
        let doing = task.map { "\u{201C}\($0)\u{201D}" } ?? "This"
        switch self {
        case .ffmpeg:
            return "\(doing) needs FFmpeg, a free tool for audio and video. Radlet can install it now, "
                + "and finds it on its own from then on."
        case .keynote, .pages, .numbers:
            return "\(doing) uses \(name), which exports its own documents. \(name) is free from Apple in the "
                + "App Store. Once it's installed, choose this again."
        case .speechLanguage:
            return "\(doing) needs the language spoken to be on this Mac. Download it by turning on Dictation "
                + "for that language in Keyboard settings, then try again."
        case .translationLanguage:
            return "\(doing) needs both languages on this Mac. Download them under Translation Languages in "
                + "Language & Region settings, then try again."
        }
    }

    /// The button that gets it.
    public var actionTitle: String {
        switch self {
        case .ffmpeg: return "Install"
        case .keynote, .pages, .numbers: return "Open App Store"
        case .speechLanguage: return "Open Keyboard Settings"
        case .translationLanguage: return "Open Language & Region"
        }
    }

    /// Whether the engine's `refresh` answer says this is here now: nil for
    /// the downloads only a conversion's helper can tell about.
    public func isMet(by refreshed: JSONValue) -> Bool? {
        refreshed[rawValue]?.bool
    }

    /// Where the button goes, for everything but FFmpeg (which installs).
    public var destination: URL? {
        switch self {
        case .ffmpeg: return nil
        case .keynote: return URL(string: "macappstore://apps.apple.com/app/id409183694")
        case .pages: return URL(string: "macappstore://apps.apple.com/app/id409201541")
        case .numbers: return URL(string: "macappstore://apps.apple.com/app/id409203825")
        case .speechLanguage: return URL(string: "x-apple.systempreferences:com.apple.Keyboard-Settings.extension")
        case .translationLanguage:
            return URL(string: "x-apple.systempreferences:com.apple.Localization-Settings.extension")
        }
    }
}

/// The FFmpeg build the app can download for Macs without Homebrew, as
/// mac/scripts/pin-ffmpeg.sh writes it into the app (ffmpeg-build.json).
public struct PinnedFFmpeg: Codable, Equatable, Sendable {
    public let version: String
    public let url: String
    public let sha256: String
    public let size: Int64

    public init(version: String, url: String, sha256: String, size: Int64) {
        self.version = version
        self.url = url
        self.sha256 = sha256
        self.size = size
    }

    /// The pin, if it's complete: an https URL and a SHA-256 to check the
    /// download against. Until a build is published and pinned, there's
    /// nothing to download.
    public static func parse(_ data: Data) -> PinnedFFmpeg? {
        guard let pin = try? JSONDecoder().decode(PinnedFFmpeg.self, from: data),
              let url = URL(string: pin.url), url.scheme == "https", url.host != nil,
              pin.sha256.count == 64, pin.sha256.allSatisfy(\.isHexDigit), pin.size > 0 else { return nil }
        return pin
    }

    /// "62 MB", for the window.
    public var sizeText: String {
        ByteCountFormatter.string(fromByteCount: size, countStyle: .file)
    }

    /// The GitHub release the download comes from, which also has its
    /// complete source: .../releases/download/<tag>/<file> becomes
    /// .../releases/tag/<tag>.
    public var releasePage: URL? {
        guard let url = URL(string: url), let host = url.host else { return nil }
        let parts = url.path.split(separator: "/").map(String.init)
        guard parts.count == 6, parts[2] == "releases", parts[3] == "download" else { return nil }
        return URL(string: "https://\(host)/\(parts[0])/\(parts[1])/releases/tag/\(parts[4])")
    }
}
