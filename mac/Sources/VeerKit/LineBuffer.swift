import Foundation

/// Splits a byte stream into newline-terminated lines (the helper
/// protocol's framing). Partial lines are kept until the rest arrives.
public struct LineBuffer {
    private var buffer = Data()
    /// Set after an over-long partial line is thrown away, so the rest of
    /// that line (up to its newline) is skipped too.
    private var discarding = false
    /// A line longer than this is garbage, not protocol: it is dropped so
    /// a misbehaving helper can't grow memory without bound.
    public let maxLineBytes: Int

    public init(maxLineBytes: Int = 16 << 20) {
        self.maxLineBytes = maxLineBytes
    }

    /// Appends bytes and returns every complete, non-empty line, without
    /// its newline.
    public mutating func append(_ data: Data) -> [Data] {
        buffer.append(data)
        var lines: [Data] = []
        var start = buffer.startIndex
        while let newline = buffer[start...].firstIndex(of: 0x0A) {
            let line = buffer[start..<newline]
            if discarding {
                discarding = false
            } else if !line.isEmpty, line.count <= maxLineBytes {
                lines.append(Data(line))
            }
            start = buffer.index(after: newline)
        }
        buffer = Data(buffer[start...])
        if buffer.count > maxLineBytes {
            buffer.removeAll()
            discarding = true
        }
        return lines
    }

    public mutating func reset() {
        buffer.removeAll()
        discarding = false
    }

    /// Bytes waiting for the end of their line.
    public var pendingCount: Int { buffer.count }
}
