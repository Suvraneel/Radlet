import Foundation

/// A small append-only log for diagnosing the drag gesture and the helper
/// on testers' Macs. It records state changes only (never every mouse
/// move) and never file names or paths. It rotates at `maxBytes`, keeping
/// one previous file.
public final class DiagnosticsLog: @unchecked Sendable {
    public static let shared = DiagnosticsLog(
        directory: FileManager.default.urls(for: .libraryDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Logs/Radlet", isDirectory: true))

    public let fileURL: URL
    public var previousURL: URL { fileURL.deletingPathExtension().appendingPathExtension("1.log") }

    private let queue = DispatchQueue(label: "com.usemonoma.radlet.log")
    private let maxBytes: Int
    private let stamp: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()

    public init(directory: URL, maxBytes: Int = 1_000_000) {
        fileURL = directory.appendingPathComponent("radlet.log")
        self.maxBytes = maxBytes
    }

    /// Appends a line like `2026-10-21T10:00:00.123Z [drag] trigger: convert`.
    public func log(_ category: String, _ message: String) {
        let line = "\(stamp.string(from: Date())) [\(category)] \(message)\n"
        queue.async { self.append(line) }
    }

    private func append(_ line: String) {
        let fm = FileManager.default
        try? fm.createDirectory(at: fileURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        if let size = (try? fm.attributesOfItem(atPath: fileURL.path))?[.size] as? Int, size >= maxBytes {
            try? fm.removeItem(at: previousURL)
            try? fm.moveItem(at: fileURL, to: previousURL)
        }
        guard let data = line.data(using: .utf8) else { return }
        if let handle = try? FileHandle(forWritingTo: fileURL) {
            defer { try? handle.close() }
            _ = try? handle.seekToEnd()
            try? handle.write(contentsOf: data)
        } else {
            try? data.write(to: fileURL)
        }
    }

    /// Waits until every queued line is written.
    public func flush() {
        queue.sync {}
    }

    /// The last `count` lines across the current and previous file.
    public func tail(_ count: Int) -> String {
        flush()
        let read = { (url: URL) -> [Substring] in
            guard let text = try? String(contentsOf: url, encoding: .utf8) else { return [] }
            return text.split(separator: "\n", omittingEmptySubsequences: true)
        }
        let lines = read(previousURL) + read(fileURL)
        return lines.suffix(count).joined(separator: "\n")
    }
}
