import CoreGraphics
import XCTest
@testable import VeerKit

final class NormRectTests: XCTestCase {
    func testTwoPointsInAnyOrderClampToThePicture() {
        let r = NormRect(from: CGPoint(x: 0.8, y: 1.4), to: CGPoint(x: -0.2, y: 0.25))
        XCTAssertEqual(r, NormRect(x: 0, y: 0.25, w: 0.8, h: 0.75))
        XCTAssertTrue(r.isUsable)
        XCTAssertFalse(NormRect(from: CGPoint(x: 0.5, y: 0.5), to: CGPoint(x: 0.502, y: 0.7)).isUsable)
    }

    func testMovingStaysInside() {
        let r = NormRect(x: 0.6, y: 0.1, w: 0.3, h: 0.3)
        XCTAssertEqual(r.moved(dx: 0.5, dy: -0.5), NormRect(x: 0.7, y: 0, w: 0.3, h: 0.3))
    }

    func testJSONNeverPassesTheEdge() throws {
        // Values that would round up past 1 if x and w were rounded alone.
        let r = NormRect(x: 0.333_333_5, y: 0, w: 0.666_666_5, h: 1)
        let x = try XCTUnwrap(r.json["x"]?.double), w = try XCTUnwrap(r.json["w"]?.double)
        XCTAssertLessThanOrEqual(x + w, 1 + 1e-9)
        XCTAssertEqual(NormRect.full.json, .object(["x": .double(0), "y": .double(0), "w": .double(1), "h": .double(1)]))
    }

    func testPixelSize() {
        let r = NormRect(x: 0.25, y: 0.25, w: 0.5, h: 0.1)
        let px = r.pixels(of: CGSize(width: 4000, height: 3000))
        XCTAssertEqual(px.width, 2000)
        XCTAssertEqual(px.height, 300)
    }
}

final class ImageFitTests: XCTestCase {
    func testFitsAndCentres() {
        let f = ImageFit.frame(content: CGSize(width: 200, height: 100), in: CGSize(width: 424, height: 424), padding: 12)
        XCTAssertEqual(f, CGRect(x: 12, y: 112, width: 400, height: 200))
    }

    func testRoundTripsBetweenViewAndFractions() {
        let f = CGRect(x: 12, y: 112, width: 400, height: 200)
        let p = ImageFit.fraction(of: CGPoint(x: 112, y: 162), in: f)
        XCTAssertEqual(p.x, 0.25, accuracy: 1e-9)
        XCTAssertEqual(p.y, 0.25, accuracy: 1e-9)
        XCTAssertEqual(ImageFit.fraction(of: CGPoint(x: -50, y: 900), in: f), CGPoint(x: 0, y: 1))
        let v = ImageFit.viewRect(NormRect(x: 0.25, y: 0.25, w: 0.5, h: 0.5), in: f)
        XCTAssertEqual(v, CGRect(x: 112, y: 162, width: 200, height: 100))
    }
}

final class CropEditorTests: XCTestCase {
    func testDrawingOnAFullPictureMakesABox() {
        var e = CropEditor(pictureSize: CGSize(width: 1000, height: 500))
        e.begin(at: CGPoint(x: 0.5, y: 0.5))
        e.update(to: CGPoint(x: 0.2, y: 0.1))
        e.end()
        XCTAssertEqual(e.rect.x, 0.2, accuracy: 1e-9)
        XCTAssertEqual(e.rect.y, 0.1, accuracy: 1e-9)
        XCTAssertEqual(e.rect.w, 0.3, accuracy: 1e-9)
        XCTAssertEqual(e.rect.h, 0.4, accuracy: 1e-9)
    }

    func testDraggingInsideMoves() {
        var e = CropEditor(pictureSize: CGSize(width: 100, height: 100))
        e.rect = NormRect(x: 0.2, y: 0.2, w: 0.4, h: 0.4)
        e.begin(at: CGPoint(x: 0.4, y: 0.4))
        e.update(to: CGPoint(x: 0.9, y: 0.5))
        e.end()
        XCTAssertEqual(e.rect.x, 0.6, accuracy: 1e-9)  // stopped at the right edge
        XCTAssertEqual(e.rect.y, 0.3, accuracy: 1e-9)
        XCTAssertEqual(e.rect.w, 0.4, accuracy: 1e-9)
    }

    func testDraggingACornerResizesFromTheOpposite() {
        var e = CropEditor(pictureSize: CGSize(width: 100, height: 100))
        e.rect = NormRect(x: 0.2, y: 0.2, w: 0.4, h: 0.4)
        e.begin(at: CGPoint(x: 0.61, y: 0.59))  // near the bottom-right corner
        e.update(to: CGPoint(x: 0.61, y: 0.59))
        XCTAssertEqual(e.rect.w, 0.4, accuracy: 1e-9)  // grabbing doesn't jump
        e.update(to: CGPoint(x: 0.91, y: 0.79))
        e.end()
        XCTAssertEqual(e.rect.x, 0.2, accuracy: 1e-9)
        XCTAssertEqual(e.rect.y, 0.2, accuracy: 1e-9)
        XCTAssertEqual(e.rect.w, 0.7, accuracy: 1e-9)
        XCTAssertEqual(e.rect.h, 0.6, accuracy: 1e-9)
    }

    func testAspectRatioIsInPixels() {
        // 2:1 picture; a square crop is half as wide as tall in fractions.
        var e = CropEditor(pictureSize: CGSize(width: 2000, height: 1000))
        e.setAspect(1)
        let px = e.rect.pixels(of: e.pictureSize)
        XCTAssertEqual(px.width, px.height)
        XCTAssertEqual(e.rect.h, 1, accuracy: 1e-9)
        XCTAssertEqual(e.rect.x, 0.25, accuracy: 1e-9)  // centred

        e.begin(at: CGPoint(x: 0.1, y: 0.1))
        e.update(to: CGPoint(x: 0.3, y: 0.9))
        e.end()
        let drawn = e.rect.pixels(of: e.pictureSize)
        XCTAssertLessThanOrEqual(abs(drawn.width - drawn.height), 1)
        XCTAssertLessThanOrEqual(e.rect.y + e.rect.h, 1 + 1e-9)
    }

    func testWideAspectOnATallPicture() {
        var e = CropEditor(pictureSize: CGSize(width: 1000, height: 2000))
        e.setAspect(16.0 / 9.0)
        let px = e.rect.pixels(of: e.pictureSize)
        XCTAssertEqual(Double(px.width) / Double(px.height), 16.0 / 9.0, accuracy: 0.01)
        XCTAssertEqual(e.rect.w, 1, accuracy: 1e-9)
        XCTAssertLessThanOrEqual(e.rect.y + e.rect.h, 1 + 1e-9)
    }
}

final class RedactEditorTests: XCTestCase {
    func testDrawSelectAndRemove() {
        var e = RedactEditor()
        e.begin(at: CGPoint(x: 0.1, y: 0.1))
        e.update(to: CGPoint(x: 0.3, y: 0.2))
        XCTAssertNotNil(e.pending)
        e.end()
        XCTAssertEqual(e.rects.count, 1)
        XCTAssertNil(e.pending)

        // A click (no drag) on empty space adds nothing.
        e.begin(at: CGPoint(x: 0.8, y: 0.8))
        e.end()
        XCTAssertEqual(e.rects.count, 1)
        XCTAssertNil(e.selected)

        e.begin(at: CGPoint(x: 0.2, y: 0.15))
        e.end()
        XCTAssertEqual(e.selected, 0)
        e.removeSelected()
        XCTAssertTrue(e.rects.isEmpty)
    }
}

final class ToolActionTests: XCTestCase {
    private func json(_ v: JSONValue) throws -> String {
        let enc = JSONEncoder()
        enc.outputFormatting = .sortedKeys
        return String(decoding: try enc.encode(v), as: UTF8.self)
    }

    /// These strings are also checked by the engine's
    /// `tool_window_actions_are_accepted` test, so the two sides agree.
    func testActionShapes() throws {
        XCTAssertEqual(try json(ToolAction.resize(.width, 1200)),
                       #"{"kind":"steps","steps":[{"op":"resize","width":1200}]}"#)
        XCTAssertEqual(try json(ToolAction.crop(NormRect(x: 0.25, y: 0, w: 0.5, h: 1))),
                       #"{"kind":"steps","steps":[{"op":"crop","rect":{"h":1,"w":0.5,"x":0.25,"y":0}}]}"#)
        XCTAssertEqual(try json(ToolAction.redact([NormRect(x: 0.5, y: 0.5, w: 0.25, h: 0.25)], style: "pixelate")),
                       #"{"kind":"steps","steps":[{"op":"redact","rects":[{"h":0.25,"w":0.25,"x":0.5,"y":0.5}],"style":"pixelate"}]}"#)
        XCTAssertEqual(try json(ToolAction.compress(level: "strong", targetBytes: 2_000_000)),
                       #"{"kind":"steps","steps":[{"level":"strong","op":"compress","target_bytes":2000000}]}"#)
        XCTAssertEqual(try json(ToolAction.splitChunks(3)),
                       #"{"kind":"split_pdf","mode":{"by":"chunks","size":3}}"#)
        XCTAssertEqual(try json(ToolAction.splitRanges("1-3, 5")),
                       #"{"kind":"split_pdf","mode":{"by":"ranges","ranges":"1-3, 5"}}"#)
        XCTAssertEqual(try json(ToolAction.splitEveryPage()),
                       #"{"kind":"split_pdf","mode":{"by":"every_page"}}"#)
        XCTAssertEqual(try json(ToolAction.pages([3, 1, 2])),
                       #"{"kind":"steps","steps":[{"op":"pages","pages":[3,1,2]}]}"#)
    }

    func testResizedSizeMatchesTheEngine() {
        let s = CGSize(width: 4000, height: 3000)
        XCTAssertEqual(ToolAction.resizedSize(s, .percent, 50), CGSize(width: 2000, height: 1500))
        XCTAssertEqual(ToolAction.resizedSize(s, .percent, 100), s)
        XCTAssertEqual(ToolAction.resizedSize(s, .max, 1000), CGSize(width: 1000, height: 750))
        XCTAssertEqual(ToolAction.resizedSize(s, .max, 9000), s)  // never enlarges
        XCTAssertEqual(ToolAction.resizedSize(s, .width, 8000), CGSize(width: 8000, height: 6000))
        XCTAssertEqual(ToolAction.resizedSize(s, .height, 300), CGSize(width: 400, height: 300))
        XCTAssertNil(ToolAction.resizedSize(s, .width, 0))
    }

    func testSlotWithActionDropsTheWindow() {
        let slot = Slot(id: "crop", label: "Crop…", symbol: "crop", group: "tool",
                        action: ToolAction.crop(.full), window: "crop")
        let ready = slot.with(action: ToolAction.crop(NormRect(x: 0, y: 0, w: 0.5, h: 0.5)))
        XCTAssertNil(ready.window)
        XCTAssertEqual(ready.id, "crop")
        XCTAssertNotEqual(ready.action, slot.action)
    }

    func testOutcomeListsEveryCreatedItem() throws {
        let line = #"{"index":0,"inputs":["/a.pdf"],"status":"done","output":"/a (split)","outputs":["/a (split)"],"bytes_in":10,"bytes_out":12,"millis":3}"#
        let o = try JSONDecoder().decode(Outcome.self, from: Data(line.utf8))
        XCTAssertEqual(o.createdItems, ["/a (split)"])
        let two = #"{"index":0,"inputs":["/a.pdf"],"status":"done","output":"/a - page 1.png","outputs":["/a - page 1.png","/a - page 2.png"],"bytes_in":10,"bytes_out":12,"millis":3}"#
        XCTAssertEqual(try JSONDecoder().decode(Outcome.self, from: Data(two.utf8)).createdItems.count, 2)
        let old = #"{"index":0,"inputs":["/a.png"],"status":"done","output":"/a.jpg","bytes_in":10,"bytes_out":5,"millis":3}"#
        XCTAssertEqual(try JSONDecoder().decode(Outcome.self, from: Data(old.utf8)).createdItems, ["/a.jpg"])
    }
}

final class PageRangesTests: XCTestCase {
    func testReadsRangesLikeTheEngine() {
        XCTAssertEqual(try PageRanges.parse("1-3, 5; 8-", count: 9).get(), [1...3, 5...5, 8...9])
        XCTAssertEqual(try PageRanges.parse(" -2 ,", count: 4).get(), [1...2])
        XCTAssertEqual(try PageRanges.parse("-", count: 3).get(), [1...3])
        // An open end with an unknown page count must not build a huge list.
        XCTAssertEqual(try PageRanges.parse("2-", count: Int(UInt32.max)).get(), [2...Int(UInt32.max)])
    }

    func testExplainsMistakes() {
        func message(_ s: String, _ n: Int) -> String? {
            if case .failure(let e) = PageRanges.parse(s, count: n) { return e.message }
            return nil
        }
        XCTAssertEqual(message("3-1", 5), "\"3-1\" isn't a page range (try 1-3, 5, 8-)")
        XCTAssertEqual(message("0", 5), "\"0\" isn't a page range (try 1-3, 5, 8-)")
        XCTAssertEqual(message("1-2-3", 5), "\"1-2-3\" isn't a page range (try 1-3, 5, 8-)")
        XCTAssertEqual(message("4-9", 5), "page 9 doesn't exist (the PDF has 5)")
        XCTAssertEqual(message(" , ", 5), "enter at least one page range")
    }

    func testChunkCount() {
        XCTAssertEqual(PageRanges.chunkCount(pages: 10, size: 3), 4)
        XCTAssertEqual(PageRanges.chunkCount(pages: 9, size: 3), 3)
        XCTAssertEqual(PageRanges.chunkCount(pages: 0, size: 3), 0)
        XCTAssertEqual(PageRanges.chunkCount(pages: Int.max, size: Int.max), 1)
    }
}

@MainActor
final class JobValueTests: XCTestCase {
    private func finish(_ p: JobProgress, _ index: Int, _ input: String, value: String?, status: String = "done") {
        var json: [String: Any] = ["index": index, "inputs": [input], "status": status, "bytes_in": 1, "millis": 1]
        if let value { json["value"] = value }
        if status != "done" { json["error"] = ["code": "nothing_to_do", "message": "no text found"] }
        let data = try! JSONSerialization.data(withJSONObject: json)
        p.handle(.finished(try! JSONDecoder().decode(Outcome.self, from: data)))
    }

    func testCopiedTextGoesToTheClipboardAsText() {
        let p = JobProgress()
        p.reset(title: "Copy text", total: 1)
        p.valueKind = .text
        finish(p, 0, "/s/receipt.png", value: "Total 12.50\nThank you")
        p.handle(.jobFinished(Summary(done: 1, skipped: 0, failed: 0, cancelled: 0), cancelled: false))
        XCTAssertEqual(p.clipboardText, "Total 12.50\nThank you")
        XCTAssertEqual(p.detail, "Text copied to the clipboard")
    }

    func testSeveralFilesAreLabelledAndMissesCounted() {
        let p = JobProgress()
        p.reset(title: "Copy text", total: 3)
        p.valueKind = .text
        finish(p, 1, "/s/b.png", value: "second")  // finishes first
        finish(p, 0, "/s/a.png", value: "first")
        finish(p, 2, "/s/blank.png", value: nil, status: "skipped")
        p.handle(.jobFinished(Summary(done: 2, skipped: 1, failed: 0, cancelled: 0), cancelled: false))
        XCTAssertEqual(p.clipboardText, "a.png\n\nfirst\n\nb.png\n\nsecond")
        XCTAssertEqual(p.detail, "Text copied to the clipboard · 1 without")
        XCTAssertTrue(p.replaceable.isEmpty)
    }

    func testChecksumsKeepTheirShape() {
        let p = JobProgress()
        p.reset(title: "sum", total: 2)
        finish(p, 0, "/s/a.dmg", value: String(repeating: "a", count: 64))
        finish(p, 1, "/s/b.dmg", value: String(repeating: "b", count: 64))
        XCTAssertEqual(p.clipboardText, "\(String(repeating: "a", count: 64))  a.dmg\n\(String(repeating: "b", count: 64))  b.dmg")
        p.valueKind = .text
        p.reset(title: "again", total: 1)
        XCTAssertEqual(p.valueKind, .checksum)  // reset forgets the kind
    }
}
