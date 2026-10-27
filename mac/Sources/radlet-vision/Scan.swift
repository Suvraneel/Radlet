// Photos of paper pages made into a scanned PDF, and text blacked out of
// PDFs for real.
//
//   radlet-vision scan <output.pdf> <photo>...
//       each page found in its photo, squared up, evened out like a
//       scanner would, with an invisible text layer
//   radlet-vision redact-pdf <input.pdf> <output.pdf> <terms.json>
//       terms.json is {"words": [...], "kinds": ["emails", "phones", "cards"]}
//
// Redaction removes the text, not just covers it. Every page's text layer,
// form fields and notes are searched, and every page is also read as a
// picture, so text in images and scans is found too. A page with a match
// is redrawn as a picture with black boxes over the matches, read again
// (anything still showing is covered), then given a fresh text layer from
// that picture, so nothing under a box can be selected, searched or
// copied. Pages with no match stay as they were. Exit 3 means none of the
// text was found.

import CoreGraphics
import CoreImage
import Foundation
import PDFKit
import Vision

// MARK: - Scanning

/// A photo of a page, cut out along the page's edges, squared up and
/// evened out. Without a page edge in sight, the whole photo is kept.
func straighten(_ image: CGImage, context: CIContext) throws -> CGImage {
    let request = VNDetectDocumentSegmentationRequest()
    // Where page finding can't run (some virtual Macs), the whole photo
    // is used.
    let found = (try? VNImageRequestHandler(cgImage: image, options: [:]).perform([request])) != nil
    var picture = CIImage(cgImage: image)
    if found, let page = request.results?.first, page.confidence > 0.5 {
        // Corners are fractions with the origin at the bottom left, as Core
        // Image measures.
        let w = CGFloat(image.width), h = CGFloat(image.height)
        let point = { (p: CGPoint) in CIVector(x: p.x * w, y: p.y * h) }
        let squared = picture.applyingFilter("CIPerspectiveCorrection", parameters: [
            "inputTopLeft": point(page.topLeft),
            "inputTopRight": point(page.topRight),
            "inputBottomLeft": point(page.bottomLeft),
            "inputBottomRight": point(page.bottomRight),
        ])
        let e = squared.extent
        if !e.isInfinite, e.width >= 64, e.height >= 64 { picture = squared }
    }
    // Shadows and uneven light lifted, text made crisp.
    if let enhancer = CIFilter(name: "CIDocumentEnhancer") {
        enhancer.setValue(picture, forKey: kCIInputImageKey)
        if let out = enhancer.outputImage, !out.extent.isInfinite { picture = out }
    }
    guard let out = context.createCGImage(picture, from: picture.extent.integral) else {
        throw Failure.failed("couldn't straighten this photo")
    }
    return out
}

func scan(_ args: [String]) throws {
    let output = URL(fileURLWithPath: args[0])
    let inputs = args.dropFirst().map { URL(fileURLWithPath: $0) }
    do {
        try writeScan(inputs, to: output)
    } catch {
        try? FileManager.default.removeItem(at: output)
        throw error
    }
}

private func writeScan(_ inputs: [URL], to output: URL) throws {
    // Read every photo first, so a bad one stops this before the PDF exists.
    let files = try inputs.map { try ImageFile($0) }
    guard let consumer = CGDataConsumer(url: output as CFURL),
          let ctx = CGContext(consumer: consumer, mediaBox: nil, nil) else {
        throw Failure.failed("couldn't create the PDF")
    }
    defer { ctx.closePDF() }
    let context = CIContext()
    for (i, file) in files.enumerated() {
        try autoreleasepool {
            guard let photo = file.upright(0, longest: 4000) else {
                throw Failure.failed("couldn't read \(file.url.lastPathComponent)")
            }
            let page = try straighten(photo, context: context)
            let size = pageSize(forPixels: CGSize(width: page.width, height: page.height))
            var box = CGRect(origin: .zero, size: size)
            ctx.beginPage(mediaBox: &box)
            ctx.interpolationQuality = .high
            ctx.draw(jpegBacked(page) ?? page, in: box)
            if let observations = try? recognize(page, .up) {
                drawInvisibleText(observations, in: ctx, size: size)
            }
            ctx.endPage()
        }
        report(Double(i + 1) / Double(files.count))
    }
}

// MARK: - Redacting text

/// What to black out.
struct RedactTerms {
    var patterns: [NSRegularExpression] = []
    /// Patterns whose matches must also pass the card number check.
    var cardPatterns: Set<Int> = []
    var phonePatterns: Set<Int> = []

    init(_ url: URL) throws {
        let spec = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any] ?? [:]
        let words = (spec["words"] as? [String] ?? [])
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
        let kinds = Set(spec["kinds"] as? [String] ?? [])
        for word in words {
            // Any run of white space in a phrase matches any other, since
            // PDF text often breaks lines inside it.
            let parts = word.precomposedStringWithCompatibilityMapping
                .split(whereSeparator: \.isWhitespace)
                .map { NSRegularExpression.escapedPattern(for: String($0)) }
            patterns.append(try NSRegularExpression(pattern: parts.joined(separator: "\\s+"), options: [.caseInsensitive]))
        }
        if kinds.contains("emails") {
            patterns.append(try NSRegularExpression(pattern: "[A-Z0-9._%+-]+@[A-Z0-9.-]+\\.[A-Z]{2,}",
                                                    options: [.caseInsensitive]))
        }
        if kinds.contains("cards") {
            cardPatterns.insert(patterns.count)
            patterns.append(try NSRegularExpression(pattern: "(?<!\\d)(?:\\d[ -]?){12,18}\\d(?!\\d)"))
        }
        if kinds.contains("phones") {
            phonePatterns.insert(patterns.count)
            patterns.append(try NSRegularExpression(pattern: "(?<![\\w+])\\+?\\(?\\d[\\d ().-]{6,}\\d(?!\\w)"))
        }
        if patterns.isEmpty { throw Failure.failed("nothing to black out was given") }
    }

    /// Ranges of `original` to black out. Matching sees compatibility forms
    /// as plain letters (the ﬁ ligature as "fi"), as people read them.
    func matches(in original: String) -> [NSRange] {
        let folded = Folded(original)
        let text = folded.text
        let whole = NSRange(location: 0, length: (text as NSString).length)
        var found: [NSRange] = []
        for (i, pattern) in patterns.enumerated() {
            for m in pattern.matches(in: text, range: whole) {
                let digits = (text as NSString).substring(with: m.range).filter(\.isNumber)
                if cardPatterns.contains(i) && !luhn(digits) { continue }
                if phonePatterns.contains(i) && !(9...15).contains(digits.count) { continue }
                if let range = folded.original(m.range) { found.append(range) }
            }
        }
        return found
    }

    /// Whether `text` holds anything to black out.
    func matches(_ text: String?) -> Bool {
        guard let text, !text.isEmpty else { return false }
        return !matches(in: text).isEmpty
    }
}

/// Text with compatibility forms folded (NFKC), remembering where each
/// folded character came from.
struct Folded {
    let text: String
    /// For each UTF-16 unit of `text`, its range in the original.
    private let origin: [NSRange]

    init(_ original: String) {
        let ns = original as NSString
        var out = ""
        var origin: [NSRange] = []
        ns.enumerateSubstrings(in: NSRange(location: 0, length: ns.length),
                               options: .byComposedCharacterSequences) { piece, range, _, _ in
            guard let piece else { return }
            let folded = piece.precomposedStringWithCompatibilityMapping
            out += folded
            origin.append(contentsOf: repeatElement(range, count: (folded as NSString).length))
        }
        text = out
        self.origin = origin
    }

    /// The original's range for a range of `text`.
    func original(_ r: NSRange) -> NSRange? {
        guard r.length > 0, r.location >= 0, r.location + r.length <= origin.count else { return nil }
        let first = origin[r.location], last = origin[r.location + r.length - 1]
        return NSRange(location: first.location, length: last.location + last.length - first.location)
    }
}

/// The check digit every card number carries.
func luhn(_ digits: String) -> Bool {
    let values = digits.compactMap(\.wholeNumberValue)
    guard (13...19).contains(values.count) else { return false }
    var sum = 0
    for (i, d) in values.reversed().enumerated() {
        let v = i % 2 == 1 ? d * 2 : d
        sum += v > 9 ? v - 9 : v
    }
    return sum % 10 == 0
}

/// A page drawn as a picture at about 200 dpi with black boxes over
/// `boxes` (in the page's own coordinates) and `pictureBoxes` (fractions
/// of the upright page, from the bottom left).
func renderRedacted(_ page: CGPDFPage, _ annotated: PDFPage?, boxes: [CGRect], pictureBoxes: [CGRect]) -> CGImage? {
    let size = uprightSize(of: page)
    guard size.width > 0, size.height > 0 else { return nil }
    var scale: CGFloat = 200 / 72
    let longest = max(size.width, size.height) * scale
    if longest > 4000 { scale *= 4000 / longest }
    let w = Int((size.width * scale).rounded()), h = Int((size.height * scale).rounded())
    guard w > 0, h > 0, let space = CGColorSpace(name: CGColorSpace.sRGB),
          let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0, space: space,
                              bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else { return nil }
    ctx.setFillColor(CGColor(gray: 1, alpha: 1))
    ctx.fill(CGRect(x: 0, y: 0, width: w, height: h))
    ctx.scaleBy(x: scale, y: scale)
    drawPage(page, annotated, in: ctx, size: size, links: false)
    ctx.setFillColor(CGColor(gray: 0, alpha: 1))
    ctx.saveGState()
    ctx.concatenate(page.getDrawingTransform(.cropBox, rect: CGRect(origin: .zero, size: size),
                                             rotate: 0, preserveAspectRatio: true))
    for box in boxes { ctx.fill(box.insetBy(dx: -1, dy: -1)) }
    ctx.restoreGState()
    for b in pictureBoxes {
        ctx.fill(CGRect(x: b.minX * size.width, y: b.minY * size.height,
                        width: b.width * size.width, height: b.height * size.height).insetBy(dx: -1, dy: -1))
    }
    return ctx.makeImage()
}

func redactPDF(_ args: [String]) throws {
    let input = URL(fileURLWithPath: args[0]), output = URL(fileURLWithPath: args[1])
    let terms = try RedactTerms(URL(fileURLWithPath: args[2]))
    do {
        try writeRedacted(input, output, terms)
    } catch {
        try? FileManager.default.removeItem(at: output)
        throw error
    }
}

/// Fractions of the upright page, from the bottom left, of the text in a
/// picture of it that matches.
func pictureMatches(_ observations: [VNRecognizedTextObservation], _ terms: RedactTerms) -> [CGRect] {
    var boxes: [CGRect] = []
    for observation in observations {
        guard let candidate = observation.topCandidates(1).first else { continue }
        let string = candidate.string
        for range in terms.matches(in: string) {
            guard let r = Range(range, in: string),
                  let box = try? candidate.boundingBox(for: r)?.boundingBox else { continue }
            boxes.append(box)
        }
    }
    return boxes
}

private func writeRedacted(_ input: URL, _ output: URL, _ terms: RedactTerms) throws {
    let (kit, cg) = try openPDF(input)
    guard let consumer = CGDataConsumer(url: output as CFURL),
          let ctx = CGContext(consumer: consumer, mediaBox: nil, nil) else {
        throw Failure.failed("couldn't create the PDF")
    }
    var blacked = 0
    defer { ctx.closePDF() }
    let count = cg.numberOfPages
    for i in stride(from: 1, through: count, by: 1) {
        try autoreleasepool {
            guard let page = cg.page(at: i) else { return }
            let annotated = kit.page(at: i - 1)
            let size = uprightSize(of: page)
            // Matches in the page's text, and in filled-in fields, notes
            // and stamps (whose whole box is covered)...
            var boxes: [CGRect] = []
            if let annotated {
                for range in terms.matches(in: annotated.string ?? "") {
                    for line in annotated.selection(for: range)?.selectionsByLine() ?? [] {
                        let b = line.bounds(for: annotated)
                        if b.width > 0, b.height > 0 { boxes.append(b) }
                    }
                }
                for note in annotated.annotations
                where terms.matches(note.widgetStringValue) || terms.matches(note.contents) {
                    boxes.append(note.bounds)
                }
            }
            // ...and in what every page shows, pictures of text included.
            var pictureBoxes: [CGRect] = []
            if let picture = render(page, annotated), let observations = try? recognize(picture, .up) {
                pictureBoxes = pictureMatches(observations, terms)
            }
            var box = CGRect(origin: .zero, size: size)
            ctx.beginPage(mediaBox: &box)
            if boxes.isEmpty && pictureBoxes.isEmpty {
                drawPage(page, annotated, in: ctx, size: size, links: true)
            } else {
                // The redrawn page is read again; anything still showing is
                // covered too.
                var redacted: CGImage?
                var observations: [VNRecognizedTextObservation] = []
                for round in 0..<3 {
                    redacted = renderRedacted(page, annotated, boxes: boxes, pictureBoxes: pictureBoxes)
                    guard let image = redacted else { break }
                    observations = (try? recognize(image, .up)) ?? []
                    let left = pictureMatches(observations, terms)
                    if left.isEmpty { break }
                    if round == 2 { throw Failure.failed("couldn't cover everything on page \(i)") }
                    pictureBoxes += left
                }
                guard let image = redacted else { throw Failure.failed("couldn't redraw page \(i)") }
                ctx.interpolationQuality = .high
                ctx.draw(jpegBacked(image) ?? image, in: box)
                drawInvisibleText(observations, in: ctx, size: size)
                blacked += boxes.count + pictureBoxes.count
            }
            ctx.endPage()
        }
        report(Double(i) / Double(count))
    }
    if blacked == 0 { throw Failure.nothingFound("none of that text is in this PDF") }
}

// MARK: - Self-test

/// One line for the self-test: the test picture scanned, and a word
/// blacked out of the searchable test PDF.
func pagesSelftest(photo: URL, pdf: URL, in dir: URL) throws -> String {
    let scanned = dir.appendingPathComponent("scanned.pdf")
    try scan([scanned.path, photo.path])
    guard PDFDocument(url: scanned)?.pageCount == 1 else { throw Failure.failed("the scanned test page is missing") }
    let terms = dir.appendingPathComponent("terms.json")
    try JSONSerialization.data(withJSONObject: ["words": ["Radlet"]]).write(to: terms)
    let redacted = dir.appendingPathComponent("redacted.pdf")
    try redactPDF([pdf.path, redacted.path, terms.path])
    guard let left = PDFDocument(url: redacted)?.string else { throw Failure.failed("the redacted test PDF is missing") }
    guard !left.localizedCaseInsensitiveContains("radlet") else {
        throw Failure.failed("the redacted test PDF still has the word in it")
    }
    return "scan, redact: ok"
}
