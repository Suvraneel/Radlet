// Documents as PDF pages, laid out by the Mac's own text system, which
// has fonts for every script (the engine's built-in writer only has the
// standard Western fonts). The engine writes the document as plain HTML
// with no outside resources; this reads it with AppKit and draws it onto
// A4 pages with TextKit, so tables, lists and backgrounds come out as
// they do in TextEdit. Links stay clickable.

import AppKit
import CoreGraphics
import Foundation

/// A4 in points, and the margin all round.
private let a4 = CGSize(width: 595.28, height: 841.89)
private let margin: CGFloat = 56

func documentPDF(_ input: URL, _ output: URL) throws {
    do {
        try writeDocumentPDF(input, output)
    } catch {
        try? FileManager.default.removeItem(at: output)
        throw error
    }
}

private func writeDocumentPDF(_ input: URL, _ output: URL) throws {
    let data = try Data(contentsOf: input)
    let options: [NSAttributedString.DocumentReadingOptionKey: Any] = [
        .documentType: NSAttributedString.DocumentType.html,
        .characterEncoding: String.Encoding.utf8.rawValue,
    ]
    let text = try NSAttributedString(data: data, options: options, documentAttributes: nil)
    guard text.length > 0 else { throw Failure.nothingFound("the document is empty") }

    let storage = NSTextStorage(attributedString: text)
    let layout = NSLayoutManager()
    storage.addLayoutManager(layout)
    let frame = CGSize(width: a4.width - 2 * margin, height: a4.height - 2 * margin)

    // One text container per page, until every glyph has a place.
    var containers: [NSTextContainer] = []
    while containers.count < 5000 {
        let container = NSTextContainer(size: frame)
        container.lineFragmentPadding = 0
        layout.addTextContainer(container)
        containers.append(container)
        let range = layout.glyphRange(for: container)
        // Nothing fits (a picture larger than the page): stop rather than
        // add empty pages for ever.
        if range.length == 0 || NSMaxRange(range) >= layout.numberOfGlyphs { break }
    }

    guard let consumer = CGDataConsumer(url: output as CFURL),
          let ctx = CGContext(consumer: consumer, mediaBox: nil, nil) else {
        throw Failure.failed("couldn't create the PDF")
    }
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: true)
    let origin = CGPoint(x: margin, y: margin)
    for (i, container) in containers.enumerated() {
        let glyphs = layout.glyphRange(for: container)
        if glyphs.length == 0 && i > 0 { break }
        var box = CGRect(origin: .zero, size: a4)
        ctx.beginPage(mediaBox: &box)
        ctx.saveGState()
        // TextKit draws top-down.
        ctx.translateBy(x: 0, y: a4.height)
        ctx.scaleBy(x: 1, y: -1)
        layout.drawBackground(forGlyphRange: glyphs, at: origin)
        layout.drawGlyphs(forGlyphRange: glyphs, at: origin)
        ctx.restoreGState()
        addLinks(layout: layout, storage: storage, container: container, glyphs: glyphs, ctx: ctx)
        ctx.endPage()
        report(Double(i + 1) / Double(containers.count))
    }
    NSGraphicsContext.restoreGraphicsState()
    ctx.closePDF()
}

/// Clickable areas over linked text (PDF coordinates run bottom-up).
private func addLinks(layout: NSLayoutManager, storage: NSTextStorage, container: NSTextContainer,
                      glyphs: NSRange, ctx: CGContext) {
    let chars = layout.characterRange(forGlyphRange: glyphs, actualGlyphRange: nil)
    storage.enumerateAttribute(.link, in: chars, options: []) { value, range, _ in
        let url: URL?
        if let u = value as? URL {
            url = u
        } else if let s = value as? String {
            url = URL(string: s)
        } else {
            url = nil
        }
        guard let target = url, let scheme = target.scheme?.lowercased(),
              ["http", "https", "mailto"].contains(scheme) else { return }
        let linkGlyphs = layout.glyphRange(forCharacterRange: range, actualCharacterRange: nil)
        layout.enumerateEnclosingRects(forGlyphRange: linkGlyphs,
                                       withinSelectedGlyphRange: NSRange(location: NSNotFound, length: 0),
                                       in: container) { rect, _ in
            let flipped = CGRect(x: rect.minX + margin,
                                 y: a4.height - margin - rect.maxY,
                                 width: rect.width, height: rect.height)
            ctx.setURL(target as CFURL, for: flipped)
        }
    }
}
