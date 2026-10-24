import XCTest
@testable import RadletKit

final class ReplacePlanTests: XCTestCase {
    private func c(_ input: String, _ output: String) -> ReplaceCandidate {
        ReplaceCandidate(input: URL(fileURLWithPath: input), output: URL(fileURLWithPath: output))
    }

    func testSameFormatTakesTheOriginalName() {
        let same = c("/p/photo.JPG", "/p/photo compressed.jpg")
        XCTAssertTrue(ReplacePlan.eligible(same))
        XCTAssertEqual(ReplacePlan.renameTarget(same), URL(fileURLWithPath: "/p/photo.JPG"))
    }

    func testNewFormatKeepsItsName() {
        let converted = c("/p/photo.png", "/p/photo.jpg")
        XCTAssertTrue(ReplacePlan.eligible(converted))
        XCTAssertNil(ReplacePlan.renameTarget(converted))
    }

    func testOtherFoldersLeaveTheOriginal() {
        XCTAssertFalse(ReplacePlan.eligible(c("/p/photo.png", "/Users/me/Downloads/photo.jpg")))
        XCTAssertFalse(ReplacePlan.eligible(c("/p/photo.png", "/p/photo.png")))
        XCTAssertNil(ReplacePlan.renameTarget(c("/p/a.jpg", "/q/a resized.jpg")))
    }

    @MainActor
    func testProgressCollectsOneToOneItems() {
        let p = JobProgress()
        p.reset(title: "t", total: 3)
        func finished(_ json: String) {
            let o = try! JSONDecoder().decode(Outcome.self, from: Data(json.utf8))
            p.handle(.finished(o))
        }
        finished(#"{"index":0,"inputs":["/p/a.png"],"status":"done","output":"/p/a.jpg","bytes_in":1,"bytes_out":1,"millis":1}"#)
        finished(#"{"index":1,"inputs":["/p/b.pdf"],"status":"done","output":"/p/b - page 1.png","outputs":["/p/b - page 1.png","/p/b - page 2.png"],"bytes_in":1,"bytes_out":1,"millis":1}"#)
        finished(#"{"index":2,"inputs":["/p/c.png"],"status":"failed","error":{"code":"x","message":"no"},"bytes_in":1,"millis":1}"#)
        XCTAssertEqual(p.replaceable, [ReplaceCandidate(input: URL(fileURLWithPath: "/p/a.png"),
                                                        output: URL(fileURLWithPath: "/p/a.jpg"))])
        XCTAssertEqual(p.outputs.count, 3)
        p.reset(title: "t", total: 1)
        XCTAssertTrue(p.replaceable.isEmpty)
    }
}

final class WatchTests: XCTestCase {
    private func e(_ name: String, _ size: Int64, dir: Bool = false) -> WatchEntry {
        WatchEntry(name: name, size: size, isDirectory: dir)
    }

    func testOnlyNewFilesThatStopGrowing() {
        var s = WatchScanner(existing: ["old.png"])
        XCTAssertEqual(s.scan([e("old.png", 10), e("new.png", 100)]), [])
        XCTAssertTrue(s.isWaiting)
        XCTAssertEqual(s.scan([e("old.png", 10), e("new.png", 200)]), [])  // still copying
        XCTAssertEqual(s.scan([e("old.png", 10), e("new.png", 200)]), ["new.png"])
        XCTAssertFalse(s.isWaiting)
        XCTAssertEqual(s.scan([e("old.png", 10), e("new.png", 200)]), [])  // only once
    }

    func testIgnoresFoldersHiddenAndPartialFiles() {
        var s = WatchScanner(existing: [])
        let entries = [e("Converted", 0, dir: true), e(".DS_Store", 6), e("movie.mp4.crdownload", 9),
                       e(".radlet-ab12-x.png", 5), e("empty.png", 0)]
        XCTAssertEqual(s.scan(entries), [])
        XCTAssertEqual(s.scan(entries), [])  // an empty file is never ready...
        for _ in 0..<WatchScanner.emptyLooks { _ = s.scan(entries) }
        XCTAssertFalse(s.isWaiting)  // ...and is eventually left alone
        XCTAssertEqual(s.scan([e("empty.png", 7)]), [])
        XCTAssertTrue(s.isWaiting)  // until it fills up
        XCTAssertEqual(s.scan([e("empty.png", 7)]), ["empty.png"])
    }

    func testAFileAddedAgainIsNew() {
        var s = WatchScanner(existing: ["a.png"])
        _ = s.scan([])
        XCTAssertEqual(s.scan([e("a.png", 5)]), [])
        XCTAssertEqual(s.scan([e("a.png", 5)]), ["a.png"])
    }

    func testRulesRoundTrip() throws {
        let rules = [WatchRule(path: "/Users/me/Drop", task: .preset("to_jpeg")),
                     WatchRule(path: "/Users/me/Web", task: .recipe("Web-ready image"), enabled: false)]
        let back = WatchRule.decodeList(WatchRule.encodeList(rules))
        XCTAssertEqual(back, rules)
        XCTAssertEqual(back[0].outputURL.path, "/Users/me/Drop/Converted")
        XCTAssertEqual(WatchRule.decodeList(Data("junk".utf8)), [])
        XCTAssertEqual(WatchTask.preset("compress").title, "Compress")
    }

    func testRecipeLookup() throws {
        let reply = try JSONValue.parse(Data(#"{"recipes":[{"name":"Web","steps":[{"op":"strip_metadata"}]}],"errors":[]}"#.utf8))
        XCTAssertEqual(WatchPresets.recipeNames(in: reply), ["Web"])
        XCTAssertEqual(WatchPresets.recipeAction(named: "Web", in: reply),
                       .object(["kind": .string("steps"), "label": .string("Web"),
                                "steps": .array([.object(["op": .string("strip_metadata")])])]))
        XCTAssertNil(WatchPresets.recipeAction(named: "Gone", in: reply))
    }
}
