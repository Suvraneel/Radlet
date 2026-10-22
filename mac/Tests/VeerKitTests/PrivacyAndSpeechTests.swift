import XCTest
@testable import VeerKit

final class PrivacyReportTests: XCTestCase {
    private let reply = #"""
    {"reports":[
      {"path":"/p/IMG_1.jpg","format":"jpeg","findings":[
        {"kind":"location","label":"Location","detail":"37.77490° N, 122.41940° W","removable":true,"latitude":37.7749,"longitude":-122.4194},
        {"kind":"device","label":"Camera","detail":"Apple iPhone 17 Pro","removable":true}]},
      {"path":"/p/memo.pdf","format":"pdf","findings":[
        {"kind":"identity","label":"Author","detail":"Riley Chen","removable":true},
        {"kind":"hidden","label":"Attached files","detail":"1 file inside the PDF","removable":false}]},
      {"path":"/p/notes.txt","format":"txt","findings":[]},
      {"path":"/p/locked.pdf","error":{"code":"invalid","message":"this PDF is password-protected"}}
    ]}
    """#

    func testDecodesReportsAndSummarises() throws {
        let reports = try PrivacyReport.list(from: try JSONValue.parse(Data(reply.utf8)))
        XCTAssertEqual(reports.map(\.name), ["IMG_1.jpg", "memo.pdf", "notes.txt", "locked.pdf"])
        XCTAssertEqual(reports[3].error?.message, "this PDF is password-protected")
        XCTAssertEqual(reports[3].findings, [])
        let place = reports[0].findings[0]
        XCTAssertEqual(place.mapsURL?.absoluteString, "https://maps.apple.com/?ll=37.7749,-122.4194&q=37.7749,-122.4194")
        XCTAssertEqual(place.symbol, "location.fill")
        XCTAssertNil(reports[0].findings[1].mapsURL)

        XCTAssertEqual(PrivacySummary.headline(reports),
                       "Location in 1 of 3 files · names or serial numbers in 1 · camera or device in 1 · and more")
        XCTAssertEqual(PrivacySummary.filesToClean(reports), ["/p/IMG_1.jpg", "/p/memo.pdf"])
        XCTAssertEqual(PrivacySummary.staying(reports), ["Attached files"])
        XCTAssertEqual(PrivacySummary.headline([reports[2]]), "Nothing revealing found.")
        XCTAssertEqual(PrivacySummary.headline([reports[0]]), "Location · camera or device")
        XCTAssertEqual(PrivacySummary.headline([reports[3]]), "Veer couldn't check these files.")
    }
}

final class SpeechAndSmartTests: XCTestCase {
    private func json(_ v: JSONValue) throws -> String {
        let enc = JSONEncoder()
        enc.outputFormatting = .sortedKeys
        return String(decoding: try enc.encode(v), as: UTF8.self)
    }

    /// Also checked by the engine's `tool_window_actions_are_accepted`.
    func testTranscribeShapes() throws {
        XCTAssertEqual(try json(ToolAction.transcribe(to: "srt", language: nil)),
                       #"{"kind":"steps","steps":[{"op":"transcribe","to":"srt"}]}"#)
        XCTAssertEqual(try json(ToolAction.transcribe(to: "vtt", language: "en-GB")),
                       #"{"kind":"steps","steps":[{"language":"en-GB","op":"transcribe","to":"vtt"}]}"#)
        XCTAssertEqual(try json(ToolAction.compress(level: "smart", targetBytes: nil)),
                       #"{"kind":"steps","steps":[{"level":"smart","op":"compress"}]}"#)
        XCTAssertTrue(ToolAction.isLanguageTag("pt_BR"))
        XCTAssertFalse(ToolAction.isLanguageTag("en US"))
        XCTAssertFalse(ToolAction.isLanguageTag("x"))
    }

    func testEditorAndDescriptions() throws {
        var f = StepFields(.object(["op": .string("transcribe"), "to": .string("txt"), "language": .string("fr-FR")]))
        XCTAssertTrue(f.hasOptions)
        XCTAssertEqual(f.subtitleFormat, "txt")
        XCTAssertEqual(f.language, "fr-FR")
        f.language = " "
        XCTAssertEqual(try f.build().get(), .object(["op": .string("transcribe"), "to": .string("txt")]))
        f.language = "fr FR"
        XCTAssertThrowsError(try f.build().get())
        XCTAssertEqual(StepText.describe(.object(["op": .string("transcribe"), "to": .string("vtt"),
                                                  "language": .string("de-DE")])), "Subtitles (WebVTT), de-DE")
        XCTAssertEqual(StepText.describe(.object(["op": .string("transcribe")])), "Subtitles (SRT)")
        XCTAssertEqual(StepText.describe(.object(["op": .string("compress"), "level": .string("smart")])),
                       "Compress (looks the same)")
        XCTAssertEqual(StepFields.levels.first, "smart")
    }
}
