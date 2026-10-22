import Foundation
import XCTest
@testable import VeerKit

final class SkinTests: XCTestCase {
    private func parse(_ css: String) -> (skin: Skin, issues: [SkinIssue]) {
        SkinCSS.parse(css, id: "file:test.css", fallbackName: "test", bases: SkinLibrary.builtInSkins)
    }

    func testBuiltInsAreCleanAndDistinct() {
        let all = SkinLibrary.builtIns
        XCTAssertEqual(all.map(\.skin.id), ["glass", "instrument", "tactical", "neon", "sunset", "porcelain"])
        for file in all {
            XCTAssertEqual(file.issues, [], file.skin.name)
            XCTAssertTrue(file.skin.isBuiltIn)
            XCTAssertEqual(file.skin.base, file.skin.id)
        }
        XCTAssertEqual(Set(all.map(\.skin.name)).count, all.count)
        XCTAssertEqual(all[0].skin, {
            var s = Skin()
            s.name = "Glass"
            s.author = "Veer"
            return s
        }(), "Glass is the defaults")
        XCTAssertEqual(SkinLibrary.defaultID, "glass")
        let instrument = all[1].skin
        XCTAssertEqual(instrument.accent, SkinColor(hex: 0xC5F05C))
        XCTAssertEqual(instrument.material, .solid)
        XCTAssertEqual(instrument.highlight, .wedge)
        XCTAssertEqual(instrument.font, .system(.mono))
        XCTAssertEqual(instrument.labelWeight, 500)
        XCTAssertEqual(instrument.tickCount, 60)
        XCTAssertEqual(all[2].skin.material, .solid, "Tactical starts from Instrument, not the default")
        XCTAssertEqual(all[2].skin.highlight, .wedge)
        let neon = all[3].skin
        XCTAssertEqual(neon.bezel2, neon.accent, "var() works in built-ins")
        XCTAssertTrue(neon.scanlines)
        XCTAssertEqual(neon.shape, .faceted)
        XCTAssertEqual(all[0].skin.accent, .system(.accent))
    }

    /// A written skin reads back exactly, for every built-in.
    func testWrittenFilesReadBack() {
        for file in SkinLibrary.builtIns {
            let css = file.skin.css()
            XCTAssertTrue(css.contains("--accent:"))
            let back = parse(css)
            XCTAssertEqual(back.issues, [], file.skin.name)
            var expected = file.skin
            expected.id = "file:test.css"
            XCTAssertEqual(back.skin, expected, file.skin.name)
        }
    }

    func testColours() {
        XCTAssertEqual(SkinColor(css: "#fff"), .rgba(1, 1, 1, 1))
        XCTAssertEqual(SkinColor(css: "#00000080"), .rgba(0, 0, 0, Double(0x80) / 255))
        XCTAssertEqual(SkinColor(css: "#F00a"), .rgba(1, 0, 0, Double(0xAA) / 255))
        XCTAssertEqual(SkinColor(css: "rgb(255, 0, 0)"), .rgba(1, 0, 0, 1))
        XCTAssertEqual(SkinColor(css: "rgba(0,0,0,0.5)"), .rgba(0, 0, 0, 0.5))
        XCTAssertEqual(SkinColor(css: "rgb(0 0 255 / 50%)"), .rgba(0, 0, 1, 0.5))
        XCTAssertEqual(SkinColor(css: "rgb(100%, 0%, 0%)"), .rgba(1, 0, 0, 1))
        XCTAssertEqual(SkinColor(css: "hsl(120, 100%, 50%)"), .rgba(0, 1, 0, 1))
        XCTAssertEqual(SkinColor(css: "hsla(0deg, 100%, 50%, 0.2)"), .rgba(1, 0, 0, 0.2))
        XCTAssertEqual(SkinColor(css: "Transparent"), .rgba(0, 0, 0, 0))
        XCTAssertEqual(SkinColor(css: "secondary-label"), .system(.secondaryLabel))
        XCTAssertEqual(SkinColor(css: "rgb(300, -4, 0)"), .rgba(1, 0, 0, 1), "clamped")
        for bad in ["#12", "#ggg", "rgb(1, 2)", "red", "rgb(a, b, c)", "", "url(x)", "hsl(1, 2, 3)"] {
            XCTAssertNil(SkinColor(css: bad), bad)
        }
        XCTAssertEqual(SkinColor(hex: 0xC5F05C).css, "#C5F05C")
        XCTAssertEqual(SkinColor.rgba(1, 1, 1, 0.09).css, "rgba(255, 255, 255, 0.09)")
        XCTAssertEqual(SkinColor.system(.accent).css, "system-accent")
    }

    func testValuesUnitsAndRanges() {
        let r = parse("""
        :root {
          --accent: #00FF9C;
          --bezel-width: 40px;
          --spring-response: 400ms;
          --stagger: 0.02s;
          --label-weight: bold;
          --highlight-opacity: 30%;
          --letter-spacing: -0.05em;
          --tick-count: 12.4;
          --label-case: UPPER;
          --font: "Avenir Next";
          --material: solid !important;
        }
        """)
        let s = r.skin
        XCTAssertEqual(s.accent, SkinColor(hex: 0x00FF9C))
        XCTAssertEqual(s.bezelWidth, 24)
        XCTAssertEqual(r.issues, [SkinIssue(line: 3, "--bezel-width is outside 0–24px; using 24px")])
        XCTAssertEqual(s.springResponse, 0.4, accuracy: 1e-9)
        XCTAssertEqual(s.stagger, 20, accuracy: 1e-9)
        XCTAssertEqual(s.labelWeight, 700)
        XCTAssertEqual(s.highlightOpacity, 0.3, accuracy: 1e-9)
        XCTAssertEqual(s.letterSpacing, -0.05, accuracy: 1e-9)
        XCTAssertEqual(s.tickCount, 12)
        XCTAssertTrue(s.uppercase)
        XCTAssertEqual(s.font, .named("Avenir Next"))
        XCTAssertEqual(s.material, .solid)
        XCTAssertEqual(s.name, "test", "no --name: the file's name")
        XCTAssertEqual(s.author, "")
        XCTAssertEqual(s.base, "glass")
        XCTAssertEqual(SkinCSS.font("system-rounded"), .system(.rounded))
        XCTAssertEqual(SkinCSS.font("\"system-mono\""), .named("system-mono"), "quoted means a family name")
    }

    func testBaseAndVariables() {
        let r = parse("""
        /* Neon, but green. */
        :root {
          --base: neon;
          --name: 'Green Neon';
          --brand: #00FF9C;
          --accent: var(--brand);
          --glow: var(--missing, rgba(0, 255, 156, 0.5));
        }
        """)
        let neon = SkinLibrary.builtIns[3].skin
        XCTAssertEqual(r.skin.name, "Green Neon")
        XCTAssertEqual(r.skin.base, "neon")
        XCTAssertEqual(r.skin.accent, SkinColor(hex: 0x00FF9C))
        XCTAssertEqual(r.skin.glow, SkinColor.rgba(0, 1, Double(156) / 255, 0.5))
        XCTAssertEqual(r.skin.scanlines, neon.scanlines, "the rest comes from the base")
        XCTAssertEqual(r.skin.font, neon.font)
        // A made-up token is reported once; the rest still applies.
        XCTAssertEqual(r.issues, [SkinIssue(line: 5, "--brand isn't a Veer token")])

        let loop = parse(":root { --a: var(--b); --b: var(--a); --accent: var(--a); }")
        XCTAssertTrue(loop.issues.contains { $0.message.contains("--accent uses var()") })
        XCTAssertEqual(loop.skin.accent, Skin().accent)

        let unknownBase = parse(":root {\n  --base: vaporwave;\n}")
        XCTAssertEqual(unknownBase.issues, [SkinIssue(line: 2, "--base “vaporwave” isn't a built-in skin; using glass")])
        XCTAssertEqual(unknownBase.skin.base, "glass")
    }

    func testMistakesAreReportedNotFatal() {
        let r = parse("""
        body { color: red; }
        :root {
          --accent: banana;
          color: blue;
          --text #fff;
          --shape: hexagon;
          --text-dim: #123456;
        }
        stray
        /* never closed
        """)
        XCTAssertEqual(r.skin.textDim, SkinColor(hex: 0x123456), "good values still apply")
        let lines = r.issues.sorted { ($0.line ?? 0) < ($1.line ?? 0) }.map(\.description)
        XCTAssertEqual(lines, [
            "Line 1: Veer reads only :root { … }; skipped “body”",
            "Line 3: --accent isn't a colour Veer knows",
            "Line 4: “color” isn't a Veer token; tokens start with --",
            "Line 5: “--text #fff” has no value (expected name: value)",
            "Line 6: --shape should be one of: round, faceted",
            "Line 9: “stray” isn't inside :root { … }",
            "Line 10: a comment is never closed with */",
        ])
        XCTAssertEqual(parse(":root { --accent: #fff;").issues.map(\.message), ["“:root” is missing its closing }"])
        XCTAssertEqual(parse(":root { --a { } }").issues.first?.message, "blocks can't be nested; skipped “:root”")
        XCTAssertEqual(parse("").issues, [])
        XCTAssertEqual(parse("}").issues.map(\.message), ["a } without a matching {"])
    }

    /// Files from anywhere: odd numbers, quotes, line endings and size
    /// never crash, hang or lose the rest of the file.
    func testHostileInput() {
        for bad in ["rgb(nan%, 0%, 0%)", "rgba(0, 0, 0, nan%)", "hsl(inf, 50%, 50%)", "hsl(0, nan%, 50%)", "rgb(inf, 0, 0)"] {
            XCTAssertNil(SkinColor(css: bad), bad)
        }
        XCTAssertEqual(SkinColor.rgba(.nan, 0, 0, .nan).css, "#000000")
        let huge = parse(":root { --tick-count: 99999999999999999999; --label-weight: -99999999999999999999; }")
        XCTAssertEqual(huge.skin.tickCount, 120)
        XCTAssertEqual(huge.skin.labelWeight, 100)
        XCTAssertEqual(huge.issues.count, 2)

        // Braces, semicolons and comment marks inside quotes are text.
        let quoted = parse(":root { --name: \"Retro {8bit} /* v2 */; ok\"; --accent: #FF0000; }")
        XCTAssertEqual(quoted.issues, [])
        XCTAssertEqual(quoted.skin.name, "Retro {8bit} /* v2 */; ok")
        XCTAssertEqual(quoted.skin.accent, SkinColor(hex: 0xFF0000))
        var odd = Skin()
        odd.name = "Retro {8bit} \"x\""
        odd.font = .named("Foo\"Bar")
        odd.labelSize = 18
        let back = parse(odd.css())
        XCTAssertEqual(back.issues, [])
        XCTAssertEqual(back.skin.name, "Retro {8bit} 'x'")
        XCTAssertEqual(back.skin.font, .named("Foo\"Bar"))
        XCTAssertEqual(back.skin.labelSize, 18)

        // Windows and old Mac line endings still give the right line.
        for newline in ["\r\n", "\r"] {
            let r = parse([":root {", "  --accent: #00FF9C;", "  --shape: blob;", "}"].joined(separator: newline))
            XCTAssertEqual(r.issues.map(\.line), [3], newline.debugDescription)
        }

        // Alpha from hex reads back exactly.
        var clear = Skin()
        clear.glow = SkinColor(css: "#00000080")!
        XCTAssertEqual(parse(clear.css()).skin.glow, clear.glow)

        // var() reaches the base's tokens, and many var()s are fine.
        let inherited = parse(":root { --base: neon; --bezel: var(--accent); }")
        XCTAssertEqual(inherited.issues, [])
        XCTAssertEqual(inherited.skin.bezel, SkinLibrary.builtIns[3].skin.accent)
        let many = parse(":root { --c: 10; --tick-count: var(--c)var(--c)var(--c)var(--c)var(--c)var(--c)var(--c)var(--c)var(--c); }")
        XCTAssertEqual(many.skin.tickCount, 120, "a long number, clamped, not a loop")
        XCTAssertFalse(many.issues.contains { $0.message.contains("loops") })

        // Big or tangled files finish quickly.
        let start = Date()
        _ = parse(String(repeating: "x", count: 64 * 1024))
        _ = parse(String(repeating: " ", count: 64 * 1024))
        var bomb = ":root { --a0: 1111111111; "
        for k in 1...7 { bomb += "--a\(k): " + String(repeating: "var(--a\(k - 1))", count: 64) + "; " }
        bomb += String(repeating: "--tick-count: var(--a7); ", count: 300) + "}"
        let blown = parse(bomb)
        XCTAssertLessThan(Date().timeIntervalSince(start), 5)
        XCTAssertTrue(blown.issues.contains { $0.message.contains("--tick-count uses var()") })
    }

    func testLibraryReadsTheSkinsFolder() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("veer-skins-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        try ":root { --base: tactical; --accent: #FF0000; }".write(to: dir.appendingPathComponent("b Red.css"), atomically: true, encoding: .utf8)
        try ":root { --name: \"Alpha\"; --bogus: 1; }".write(to: dir.appendingPathComponent("a.css"), atomically: true, encoding: .utf8)
        try "not a skin".write(to: dir.appendingPathComponent("notes.txt"), atomically: true, encoding: .utf8)
        try String(repeating: "/* x */", count: 10_000).write(to: dir.appendingPathComponent("c huge.css"), atomically: true, encoding: .utf8)
        try ":root {}".write(to: dir.appendingPathComponent(".hidden.css"), atomically: true, encoding: .utf8)

        let files = SkinLibrary.load(directory: dir)
        XCTAssertEqual(files.map(\.id), ["file:a.css", "file:b Red.css", "file:c huge.css"])
        XCTAssertEqual(files[0].skin.name, "Alpha")
        XCTAssertEqual(files[0].issues.count, 1)
        XCTAssertEqual(files[1].skin.name, "b Red")
        XCTAssertEqual(files[1].skin.shape, .faceted)
        XCTAssertEqual(files[1].skin.accent, SkinColor(hex: 0xFF0000))
        XCTAssertFalse(files[1].skin.isBuiltIn)
        XCTAssertEqual(files[2].issues.first?.message, "the file is over 64 KB, so Veer skipped it")
        XCTAssertEqual(SkinLibrary.signature(directory: dir).count, 4)
        XCTAssertEqual(SkinLibrary.load(directory: dir.appendingPathComponent("missing")), [])

        // A link to a file over the limit is skipped like the file itself.
        let big = dir.appendingPathComponent("big.txt")
        try String(repeating: "x", count: 70_000).write(to: big, atomically: true, encoding: .utf8)
        try FileManager.default.createSymbolicLink(at: dir.appendingPathComponent("d link.css"), withDestinationURL: big)
        let linked = SkinLibrary.load(directory: dir).first { $0.id == "file:d link.css" }
        XCTAssertEqual(linked?.issues.first?.message, "the file is over 64 KB, so Veer skipped it")

        XCTAssertEqual(SkinLibrary.newFileURL(in: dir, name: "a").lastPathComponent, "a 2.css")
        XCTAssertEqual(SkinLibrary.newFileURL(in: dir, name: "My/Skin").lastPathComponent, "My-Skin.css")
        XCTAssertEqual(SkinLibrary.newFileURL(in: dir, name: " ").lastPathComponent, "My Skin.css")
    }

    func testCompactFilesHoldOnlyChanges() {
        for builtIn in SkinLibrary.builtInSkins {
            let plain = builtIn.compactCSS()
            XCTAssertEqual(plain.split(separator: "\n").count, 5, plain)
            XCTAssertTrue(plain.contains("--base: \(builtIn.id);"))

            var mine = builtIn
            mine.name = "Mine \"quoted\"\nname"
            mine.author = ""
            mine.accent = SkinColor(hex: 0x00FF9C, alpha: 0.5)
            mine.shape = mine.shape == .round ? .faceted : .round
            mine.springResponse = 0.47
            mine.tickCount = 33
            mine.font = .named("Avenir Next")
            let css = mine.compactCSS()
            XCTAssertFalse(css.contains("--author"))
            XCTAssertEqual(css.split(separator: "\n").count, 9, css)
            let back = parse(css)
            XCTAssertEqual(back.issues, [], css)
            XCTAssertEqual(back.skin.name, "Mine 'quoted'name")
            XCTAssertEqual(back.skin.base, builtIn.id)
            for token in SkinCSS.editable {
                XCTAssertEqual(token.value(in: back.skin), token.value(in: mine), "\(builtIn.id) \(token.name)")
            }
        }
        // A file's own skin compacts against its built-in base.
        let file = parse(":root { --base: neon; --accent: #00FF00; }").skin
        XCTAssertTrue(file.compactCSS().contains("--accent: #00FF00;"))
        XCTAssertTrue(file.compactCSS().contains("--base: neon;"))
        var lost = Skin()
        lost.base = "gone"
        XCTAssertTrue(lost.compactCSS().contains("--base: glass;"))
        XCTAssertEqual(SkinLibrary.base(of: lost).id, "glass")
        XCTAssertEqual(SkinLibrary.base(of: file).id, "neon")
        XCTAssertTrue(SkinLibrary.defaultDirectory.path.hasSuffix("Veer/Skins"))
    }

    func testTokensDescribeThemselves() {
        let tokens = Dictionary(uniqueKeysWithValues: SkinCSS.editable.map { ($0.name, $0) })
        XCTAssertEqual(SkinCSS.editable.count, SkinCSS.tokens.count - 3)
        XCTAssertNil(tokens["--name"])
        XCTAssertNil(tokens["--base"])
        XCTAssertEqual(tokens["--accent"]?.kind, .color)
        XCTAssertEqual(tokens["--font"]?.kind, .font)
        XCTAssertEqual(tokens["--shape"]?.kind, .choice(["round", "faceted"]))
        XCTAssertEqual(tokens["--label-case"]?.kind, .choice(["none", "upper"]))
        XCTAssertEqual(tokens["--tick-count"]?.kind, .number(0...120, unit: "", step: 1))
        XCTAssertEqual(tokens["--label-weight"]?.kind, .number(100...900, unit: "", step: 100))
        XCTAssertEqual(tokens["--spring-response"]?.kind, .number(0.1...1.2, unit: "s", step: 0.01))

        // Every token reads back what it writes, for every built-in.
        for skin in SkinLibrary.builtInSkins {
            for token in SkinCSS.editable {
                var copy = Skin()
                XCTAssertNil(token.apply(token.value(in: skin), to: &copy), "\(skin.id) \(token.name)")
                XCTAssertEqual(token.value(in: copy), token.value(in: skin), "\(skin.id) \(token.name)")
                if case .number(let range, _, let step) = token.kind {
                    XCTAssertTrue(step > 0)
                    XCTAssertTrue(range.contains(token.number(in: skin) ?? .nan), "\(skin.id) \(token.name)")
                } else {
                    XCTAssertNil(token.number(in: skin))
                }
            }
        }

        var skin = Skin()
        let spring = tokens["--spring-response"]!
        XCTAssertEqual(spring.number(in: skin), 0.25)
        XCTAssertEqual(spring.text(for: 0.4), "0.4s")
        XCTAssertNil(spring.apply("400ms", to: &skin))
        XCTAssertEqual(skin.springResponse, 0.4)
        let ticks = tokens["--tick-count"]!
        XCTAssertNotNil(ticks.apply("200", to: &skin), "clamped, and says so")
        XCTAssertEqual(skin.tickCount, 120)
        let accent = tokens["--accent"]!
        XCTAssertNotNil(accent.apply("#zzz", to: &skin))
        XCTAssertEqual(skin.accent, Skin().accent, "unreadable values change nothing")
        XCTAssertNil(accent.apply("system-accent", to: &skin))
        XCTAssertEqual(skin.accent, .system(.accent))
    }
}
