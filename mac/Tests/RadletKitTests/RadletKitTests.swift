import AppKit
import XCTest
@testable import RadletKit

final class WheelGeometryTests: XCTestCase {
    /// AppKit's y-up points reach the right slots: up is slot 0 and the
    /// ring runs clockwise.
    func testAppKitPointsMapToTheRightSlots() {
        let spec = WheelSpec(style: .dial, count: 4)
        let c = spec.size / 2
        let cases: [(CGPoint, Int?)] = [
            (CGPoint(x: c, y: c + 120), 0), (CGPoint(x: c + 120, y: c), 1),
            (CGPoint(x: c, y: c - 120), 2), (CGPoint(x: c - 120, y: c), 3), (CGPoint(x: c, y: c), nil),
        ]
        for (point, slot) in cases {
            var t = WheelTracker(spec: spec, presets: [])
            let o = WheelGeometry.offset(appKitPoint: point, side: spec.size)
            t.move(dx: o.dx, dy: o.dy, at: 0)
            XCTAssertEqual(t.pick?.slot, slot, "\(point)")
        }
    }

    func testModifierKeysPickTheWheel() {
        XCTAssertEqual(WheelMode.from([.shift]), .convert)
        XCTAssertEqual(WheelMode.from([.shift, .command]), .tools)
        XCTAssertEqual(WheelMode.from([.command]), .tools)
        XCTAssertEqual(WheelMode.from([.control]), .recipes)
        XCTAssertNil(WheelMode.from([]))
        XCTAssertNil(WheelMode.from([.option]))
        XCTAssertEqual(WheelMode.recipes.next, .convert)
    }
}

final class LineBufferTests: XCTestCase {
    func testLinesSplitAcrossChunks() {
        var b = LineBuffer()
        XCTAssertEqual(b.append(Data("{\"id\":1".utf8)), [])
        XCTAssertEqual(b.pendingCount, 7)
        let lines = b.append(Data("}\n{\"id\":2}\n\n{\"id\"".utf8))
        XCTAssertEqual(lines.map { String(decoding: $0, as: UTF8.self) }, ["{\"id\":1}", "{\"id\":2}"])
        XCTAssertEqual(b.append(Data(":3}\n".utf8)).count, 1)
        XCTAssertEqual(b.pendingCount, 0)
    }

    func testOverlongLinesAreDropped() {
        var b = LineBuffer(maxLineBytes: 8)
        XCTAssertEqual(b.append(Data("0123456789\nok\n".utf8)).map { String(decoding: $0, as: UTF8.self) }, ["ok"])
        _ = b.append(Data(repeating: 0x41, count: 20))
        XCTAssertEqual(b.pendingCount, 0, "garbage without a newline is discarded")
        // The tail of that over-long line is dropped too, not returned.
        XCTAssertEqual(b.append(Data("AAA\nok\n".utf8)).map { String(decoding: $0, as: UTF8.self) }, ["ok"])
    }
}

final class HelpersTests: XCTestCase {
    private func slot(_ id: String) -> Slot {
        Slot(id: id, label: id, symbol: "photo", group: "convert", action: .null)
    }

    func testUsageOrderingIsStable() {
        let slots = ["a", "b", "c", "d"].map(slot)
        XCTAssertEqual(orderByUsage(slots, counts: [:]).map(\.id), ["a", "b", "c", "d"])
        XCTAssertEqual(orderByUsage(slots, counts: ["c": 3, "b": 3, "d": 1]).map(\.id), ["b", "c", "d", "a"])
    }

    func testFileDescriptionsHideNames() {
        let urls = ["/Users/me/Secret Plans.png", "/x/b.PNG", "/x/c.jpg", "/x/README"].map { URL(fileURLWithPath: $0) }
        let text = describeFiles(urls)
        XCTAssertEqual(text, "4 files (png ×2, jpg, no extension)")
        XCTAssertFalse(text.contains("Secret"))
        XCTAssertEqual(describeFiles([]), "no files")
    }
}

final class DiagnosticsLogTests: XCTestCase {
    func testTailAndRotation() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: dir) }
        let log = DiagnosticsLog(directory: dir, maxBytes: 400)
        for i in 0..<30 {
            log.log("test", "line \(i)")
        }
        let tail = log.tail(5)
        XCTAssertEqual(tail.split(separator: "\n").count, 5)
        XCTAssertTrue(tail.hasSuffix("[test] line 29"), tail)
        XCTAssertTrue(FileManager.default.fileExists(atPath: log.previousURL.path), "rotated at maxBytes")
        let size = try XCTUnwrap(FileManager.default.attributesOfItem(atPath: log.fileURL.path)[.size] as? Int)
        XCTAssertLessThan(size, 400 + 100)
    }
}
