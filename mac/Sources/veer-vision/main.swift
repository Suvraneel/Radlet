// veer-vision: on-device text recognition, subject masks and searchable
// PDFs for the Veer engine, using Apple's Vision framework and PDFKit.
// Nothing leaves the Mac.
//
//   veer-vision text <input> <output.txt>        image or PDF
//   veer-vision mask <input> <output.png>        foreground mask (white = keep)
//   veer-vision searchable <input> <output.pdf>  image or PDF, with an
//                                                invisible text layer
//   veer-vision transcribe <language|auto> <output.json> <chunk.wav>...
//                                                speech to timed words
//   veer-vision document-pdf <input.html> <output.pdf>
//                                                a document laid out on
//                                                A4 pages (DocumentPDF.swift)
//   veer-vision selftest                         checks this Mac can do them
//   veer-vision --version
//
// Exit 0 means done, 3 means nothing was found (no text, no subject), and
// anything else is a failure. The last line on stderr explains. Lines like
// "progress 0.42" on stdout report progress. The engine side of this
// contract is crates/veer-core/src/engines/vision.rs.

import CoreGraphics
import CoreText
import CoreVideo
import Foundation
import ImageIO
import PDFKit
import Speech
import UniformTypeIdentifiers
import Vision

enum Failure: Error {
    /// Ran fine, found nothing: exit 3.
    case nothingFound(String)
    case failed(String)
}

func report(_ f: Double) {
    print(String(format: "progress %.3f", min(max(f, 0), 1)))
    fflush(stdout)
}

// MARK: - Reading

/// Characters other than white space.
func visibleCount(_ s: String) -> Int {
    s.unicodeScalars.filter { !CharacterSet.whitespacesAndNewlines.contains($0) }.count
}

enum Limits {
    /// A text layer at least this long is the page's real text. Shorter
    /// ones are often just a scanner's stamp, a fax header or Bates numbers
    /// over a scanned page, so those pages are recognised too.
    static let realTextLayer = 200
}

/// Decided by the file's first bytes, as the engine does; the extension
/// only counts when the file can't be read.
func isPDF(_ url: URL) -> Bool {
    let byName = url.pathExtension.lowercased() == "pdf"
    guard let handle = try? FileHandle(forReadingFrom: url) else { return byName }
    defer { try? handle.close() }
    guard let head = try? handle.read(upToCount: 1024) else { return byName }
    let marker = Data("%PDF-".utf8)
    return head.starts(with: marker) || (byName && head.range(of: marker) != nil)
}

/// An image file. Multi-page TIFFs (fax and document scanners) have one
/// page per frame; other formats are read as one picture.
struct ImageFile {
    let url: URL
    let source: CGImageSource
    let type: String
    let pages: Int

    init(_ url: URL) throws {
        guard let src = CGImageSourceCreateWithURL(url as CFURL, nil), CGImageSourceGetCount(src) > 0 else {
            throw Failure.failed("couldn't read this image")
        }
        self.url = url
        source = src
        type = (CGImageSourceGetType(src) as String?) ?? ""
        pages = type == UTType.tiff.identifier ? CGImageSourceGetCount(src) : 1
    }

    /// A page and the EXIF orientation that makes it upright.
    func page(_ i: Int) throws -> (CGImage, CGImagePropertyOrientation) {
        guard let image = CGImageSourceCreateImageAtIndex(source, i, nil) else {
            throw Failure.failed("couldn't read this image")
        }
        let props = CGImageSourceCopyPropertiesAtIndex(source, i, nil) as? [CFString: Any]
        let raw = (props?[kCGImagePropertyOrientation] as? NSNumber)?.uint32Value ?? 1
        return (image, CGImagePropertyOrientation(rawValue: raw) ?? .up)
    }

    /// A page decoded upright, at full size.
    func upright(_ i: Int, longest: Int) -> CGImage? {
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: longest,
        ]
        return CGImageSourceCreateThumbnailAtIndex(source, i, options as CFDictionary)
    }

    /// A page as it should go into a PDF: upright, and compact. An upright
    /// JPEG goes in as the same JPEG; photos in other formats, or turned
    /// upright, are stored as JPEG; everything else is kept losslessly.
    func pageForPDF(_ i: Int) throws -> CGImage {
        let (image, orientation) = try page(i)
        let jpeg = type == UTType.jpeg.identifier
        if orientation == .up && jpeg { return image }
        var upright = image
        if orientation != .up {
            guard let turned = self.upright(i, longest: max(image.width, image.height)) else {
                throw Failure.failed("couldn't turn this image upright")
            }
            upright = turned
        }
        let photoTypes = [UTType.jpeg.identifier, UTType.heic.identifier, UTType.heif.identifier, "public.avif"]
        let opaque = [CGImageAlphaInfo.none, .noneSkipLast, .noneSkipFirst].contains(upright.alphaInfo)
        if photoTypes.contains(type) && opaque { return jpegBacked(upright) ?? upright }
        return upright
    }
}

/// A page's visible size in points, with its rotation applied.
func uprightSize(of page: CGPDFPage) -> CGSize {
    let box = page.getBoxRect(.cropBox)
    return page.rotationAngle % 180 == 0 ? box.size : CGSize(width: box.height, height: box.width)
}

/// Draws a page upright into a rectangle of its upright size, with its
/// annotations: signatures, stamps, highlights and filled-in form fields
/// live there, and drawPDFPage leaves them out. With `links`, web links
/// stay clickable.
func drawPage(_ page: CGPDFPage, _ annotated: PDFPage?, in ctx: CGContext, size: CGSize, links: Bool) {
    let transform = page.getDrawingTransform(.cropBox, rect: CGRect(origin: .zero, size: size),
                                             rotate: 0, preserveAspectRatio: true)
    let annotations = annotated?.annotations ?? []
    ctx.saveGState()
    ctx.concatenate(transform)
    ctx.clip(to: page.getBoxRect(.cropBox))
    ctx.drawPDFPage(page)
    for annotation in annotations where annotation.shouldDisplay {
        annotation.draw(with: .cropBox, in: ctx)
    }
    ctx.restoreGState()
    guard links else { return }
    for annotation in annotations {
        if let url = annotation.url {
            ctx.setURL(url as CFURL, for: annotation.bounds.applying(transform))
        }
    }
}

/// A page rendered for recognition: about 300 dpi, at most 5,000 pixels.
func render(_ page: CGPDFPage, _ annotated: PDFPage?) -> CGImage? {
    let size = uprightSize(of: page)
    guard size.width > 0, size.height > 0 else { return nil }
    var scale: CGFloat = 300 / 72
    let longest = max(size.width, size.height) * scale
    if longest > 5000 { scale *= 5000 / longest }
    let w = Int((size.width * scale).rounded()), h = Int((size.height * scale).rounded())
    guard w > 0, h > 0, let space = CGColorSpace(name: CGColorSpace.sRGB),
          let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0, space: space,
                              bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else { return nil }
    ctx.setFillColor(CGColor(gray: 1, alpha: 1))
    ctx.fill(CGRect(x: 0, y: 0, width: w, height: h))
    ctx.scaleBy(x: scale, y: scale)
    drawPage(page, annotated, in: ctx, size: size, links: false)
    return ctx.makeImage()
}

func openPDF(_ input: URL) throws -> (PDFDocument, CGPDFDocument) {
    guard let kit = PDFDocument(url: input), let cg = CGPDFDocument(input as CFURL) else {
        throw Failure.failed("couldn't open this PDF")
    }
    if kit.isLocked || (cg.isEncrypted && !cg.isUnlocked) {
        throw Failure.failed("this PDF is password-protected")
    }
    return (kit, cg)
}

// MARK: - Recognition

func recognize(_ image: CGImage, _ orientation: CGImagePropertyOrientation) throws -> [VNRecognizedTextObservation] {
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.usesLanguageCorrection = true
    request.automaticallyDetectsLanguage = true
    try VNImageRequestHandler(cgImage: image, orientation: orientation, options: [:]).perform([request])
    return request.results ?? []
}

func lines(_ observations: [VNRecognizedTextObservation]) -> String {
    observations.compactMap { $0.topCandidates(1).first?.string }.joined(separator: "\n")
}

func text(_ input: URL, _ output: URL) throws {
    var parts: [String] = []
    var failedPages = 0
    if isPDF(input) {
        let (kit, cg) = try openPDF(input)
        let count = kit.pageCount
        for i in 0..<count {
            autoreleasepool {
                let annotated = kit.page(at: i)
                let layer = annotated?.string ?? ""
                var best = layer
                // A page that is (mostly) a picture: recognise it, and keep
                // whichever text is fuller.
                if visibleCount(layer) < Limits.realTextLayer, let page = cg.page(at: i + 1),
                   let image = render(page, annotated) {
                    if let observations = try? recognize(image, .up) {
                        let recognised = lines(observations)
                        if visibleCount(recognised) > visibleCount(layer) { best = recognised }
                    } else {
                        failedPages += 1
                    }
                }
                parts.append(best)
            }
            report(Double(i + 1) / Double(count))
        }
    } else {
        let file = try ImageFile(input)
        for i in 0..<file.pages {
            try autoreleasepool {
                let (image, orientation) = try file.page(i)
                if let observations = try? recognize(image, orientation) {
                    parts.append(lines(observations))
                } else {
                    failedPages += 1
                }
            }
            if file.pages > 1 { report(Double(i + 1) / Double(file.pages)) }
        }
    }
    let joined = parts.map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
        .filter { !$0.isEmpty }
        .joined(separator: "\n\n")
    if joined.isEmpty {
        if failedPages > 0 { throw Failure.failed("text recognition failed on this Mac") }
        throw Failure.nothingFound("no text found")
    }
    try (joined + "\n").write(to: output, atomically: false, encoding: .utf8)
}

// MARK: - Subject mask

func mask(_ input: URL, _ output: URL) throws {
    let (image, orientation) = try ImageFile(input).page(0)
    let request = VNGenerateForegroundInstanceMaskRequest()
    let handler = VNImageRequestHandler(cgImage: image, orientation: orientation, options: [:])
    try handler.perform([request])
    guard let result = request.results?.first, !result.allInstances.isEmpty else {
        throw Failure.nothingFound("no subject found")
    }
    let buffer = try result.generateScaledMaskForImage(forInstances: result.allInstances, from: handler)
    try writeGray(buffer, to: output)
}

/// Saves a one-channel mask (float or 8-bit) as an 8-bit grey PNG.
func writeGray(_ buffer: CVPixelBuffer, to url: URL) throws {
    CVPixelBufferLockBaseAddress(buffer, .readOnly)
    defer { CVPixelBufferUnlockBaseAddress(buffer, .readOnly) }
    let w = CVPixelBufferGetWidth(buffer), h = CVPixelBufferGetHeight(buffer)
    let rowBytes = CVPixelBufferGetBytesPerRow(buffer)
    guard let base = CVPixelBufferGetBaseAddress(buffer), w > 0, h > 0 else {
        throw Failure.failed("the subject mask is empty")
    }
    var gray = [UInt8](repeating: 0, count: w * h)
    switch CVPixelBufferGetPixelFormatType(buffer) {
    case kCVPixelFormatType_OneComponent32Float:
        for y in 0..<h {
            let row = (base + y * rowBytes).assumingMemoryBound(to: Float32.self)
            for x in 0..<w { gray[y * w + x] = UInt8((min(max(row[x], 0), 1) * 255).rounded()) }
        }
    case kCVPixelFormatType_OneComponent8:
        for y in 0..<h {
            let row = (base + y * rowBytes).assumingMemoryBound(to: UInt8.self)
            for x in 0..<w { gray[y * w + x] = row[x] }
        }
    default:
        throw Failure.failed("unexpected mask format \(CVPixelBufferGetPixelFormatType(buffer))")
    }
    guard let provider = CGDataProvider(data: Data(gray) as CFData),
          let image = CGImage(width: w, height: h, bitsPerComponent: 8, bitsPerPixel: 8, bytesPerRow: w,
                              space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGBitmapInfo(rawValue: 0),
                              provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent),
          let dest = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else {
        throw Failure.failed("couldn't write the mask")
    }
    CGImageDestinationAddImage(dest, image, nil)
    guard CGImageDestinationFinalize(dest) else { throw Failure.failed("couldn't write the mask") }
}

// MARK: - Searchable PDF

/// Writes each recognised line as invisible text over where it appears,
/// stretched to the line's width, so search, selection and copy line up
/// with the picture.
func drawInvisibleText(_ observations: [VNRecognizedTextObservation], in ctx: CGContext, size: CGSize) {
    // Helvetica's ascent plus descent at 1 pt, to size text to a line box.
    let unit = CTFontCreateWithName("Helvetica" as CFString, 1, nil)
    let em = CTFontGetAscent(unit) + CTFontGetDescent(unit)
    ctx.saveGState()
    ctx.setTextDrawingMode(.invisible)
    for observation in observations {
        guard let string = observation.topCandidates(1).first?.string, !string.isEmpty else { continue }
        let b = observation.boundingBox  // normalised, origin at the bottom left like PDF
        let rect = CGRect(x: b.minX * size.width, y: b.minY * size.height,
                          width: b.width * size.width, height: b.height * size.height)
        guard rect.width > 1, rect.height > 1 else { continue }
        let font = CTFontCreateWithName("Helvetica" as CFString, rect.height / em, nil)
        let attributed = NSAttributedString(string: string,
                                            attributes: [NSAttributedString.Key(kCTFontAttributeName as String): font])
        let line = CTLineCreateWithAttributedString(attributed as CFAttributedString)
        let width = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
        guard width > 0 else { continue }
        ctx.textMatrix = CGAffineTransform(scaleX: rect.width / width, y: 1)
        ctx.textPosition = CGPoint(x: rect.minX, y: rect.minY + CTFontGetDescent(font))
        CTLineDraw(line, ctx)
    }
    ctx.restoreGState()
}

/// Page size for an image, as the engine sizes image pages: the longest
/// side fits an A4 page length, small images keep 72 dpi.
func pageSize(forPixels px: CGSize) -> CGSize {
    let dpi = max(max(px.width, px.height) / 11.69, 72)
    return CGSize(width: max(px.width * 72 / dpi, 1), height: max(px.height * 72 / dpi, 1))
}

/// JPEG-backed copy of an image, so the PDF stores it compactly.
func jpegBacked(_ image: CGImage) -> CGImage? {
    let data = NSMutableData()
    guard let dest = CGImageDestinationCreateWithData(data as CFMutableData, UTType.jpeg.identifier as CFString,
                                                      1, nil) else {
        return nil
    }
    CGImageDestinationAddImage(dest, image, [kCGImageDestinationLossyCompressionQuality: 0.9] as CFDictionary)
    guard CGImageDestinationFinalize(dest), let provider = CGDataProvider(data: data as CFData) else { return nil }
    return CGImage(jpegDataProviderSource: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
}

/// Writes `input` as a PDF with an invisible text layer. Nothing is left at
/// `output` if this fails or finds no text.
func searchable(_ input: URL, _ output: URL) throws {
    do {
        try writeSearchable(input, output)
    } catch {
        try? FileManager.default.removeItem(at: output)
        throw error
    }
}

private func writeSearchable(_ input: URL, _ output: URL) throws {
    // Open the input before creating anything.
    let pdf: (kit: PDFDocument, cg: CGPDFDocument)?
    let image: ImageFile?
    if isPDF(input) {
        pdf = try openPDF(input)
        image = nil
    } else {
        pdf = nil
        image = try ImageFile(input)
    }
    guard let consumer = CGDataConsumer(url: output as CFURL),
          let ctx = CGContext(consumer: consumer, mediaBox: nil, nil) else {
        throw Failure.failed("couldn't create the PDF")
    }
    var recognised = 0
    var alreadyText = 0
    var failedPages = 0
    defer { ctx.closePDF() }

    if let opened = pdf {
        let kit = opened.kit, cg = opened.cg
        let count = cg.numberOfPages
        for i in stride(from: 1, through: count, by: 1) {
            autoreleasepool {
                guard let page = cg.page(at: i) else { return }
                let annotated = kit.page(at: i - 1)
                let size = uprightSize(of: page)
                var box = CGRect(origin: .zero, size: size)
                ctx.beginPage(mediaBox: &box)
                drawPage(page, annotated, in: ctx, size: size, links: true)
                // Pages with a real text layer keep it as it is.
                if visibleCount(annotated?.string ?? "") >= Limits.realTextLayer {
                    alreadyText += 1
                } else if let picture = render(page, annotated) {
                    if let observations = try? recognize(picture, .up) {
                        recognised += observations.count
                        drawInvisibleText(observations, in: ctx, size: size)
                    } else {
                        failedPages += 1
                    }
                }
                ctx.endPage()
            }
            report(Double(i) / Double(count))
        }
    } else if let file = image {
        for i in 0..<file.pages {
            try autoreleasepool {
                let picture = try file.pageForPDF(i)
                let size = pageSize(forPixels: CGSize(width: picture.width, height: picture.height))
                var box = CGRect(origin: .zero, size: size)
                ctx.beginPage(mediaBox: &box)
                ctx.interpolationQuality = .high
                ctx.draw(picture, in: box)
                if let observations = try? recognize(picture, .up) {
                    recognised += observations.count
                    drawInvisibleText(observations, in: ctx, size: size)
                } else {
                    failedPages += 1
                }
                ctx.endPage()
            }
            if file.pages > 1 { report(Double(i + 1) / Double(file.pages)) }
        }
    }

    if recognised == 0 {
        if failedPages > 0 { throw Failure.failed("text recognition failed on this Mac") }
        throw Failure.nothingFound(alreadyText > 0 ? "this PDF is already searchable" : "no text found")
    }
}

// MARK: - Speech

/// The recogniser for a language tag ("en-US", or "auto" for the Mac's
/// language), falling back to another region of the same language.
func speechRecognizer(_ tag: String) throws -> SFSpeechRecognizer {
    let wanted = tag == "auto" ? Locale.current : Locale(identifier: tag)
    let name = Locale.current.localizedString(forIdentifier: wanted.identifier) ?? wanted.identifier
    var recognizer = SFSpeechRecognizer(locale: wanted)
    if recognizer.map({ !$0.supportsOnDeviceRecognition }) ?? true,
       let language = wanted.language.languageCode?.identifier {
        // Another region of the same language, preferring one this Mac can
        // run on-device, then the language's usual region (en-US, de-DE).
        let candidates = SFSpeechRecognizer.supportedLocales()
            .filter { $0.language.languageCode?.identifier == language }
            .sorted { $0.identifier < $1.identifier }
            .compactMap { SFSpeechRecognizer(locale: $0) }
        let usual = Locale.Language(identifier: language).maximalIdentifier
        let onDevice = candidates.filter(\.supportsOnDeviceRecognition)
        let best = onDevice.first { usual.hasSuffix("-" + ($0.locale.region?.identifier ?? "?")) }
            ?? onDevice.first
            ?? candidates.first { usual.hasSuffix("-" + ($0.locale.region?.identifier ?? "?")) }
            ?? candidates.first
        if let best { recognizer = best }
    }
    guard let recognizer else { throw Failure.failed("Speech recognition doesn't support \(name) yet.") }
    guard recognizer.supportsOnDeviceRecognition else {
        throw Failure.failed("On-device speech recognition for \(name) isn't on this Mac yet. "
                             + "Turn on Dictation for \(name) in System Settings → Keyboard to download it.")
    }
    return recognizer
}

/// Checks permission without asking: asking from a helper could stop it,
/// so the app asks first (Make Subtitles), and the helper only checks.
func requireSpeechPermission() throws {
    switch SFSpeechRecognizer.authorizationStatus() {
    case .authorized:
        return
    case .denied, .restricted:
        throw Failure.failed("Speech recognition is turned off for Veer. Turn it on in System Settings → "
                             + "Privacy & Security → Speech Recognition.")
    default:
        throw Failure.failed("Speech recognition needs permission for the app that started Veer. "
                             + "Use Make Subtitles in the Veer app, which asks for it.")
    }
}

/// Collects one recognition result; handlers run on the main queue, which
/// the run loop below services.
final class SpeechResult {
    var words: [[String: Any]] = []
    var error: Error?
    var finished = false
}

/// Words of one chunk of sound, with start times and durations.
func recognizeSpeech(_ url: URL, with recognizer: SFSpeechRecognizer) throws -> [[String: Any]] {
    let request = SFSpeechURLRecognitionRequest(url: url)
    request.requiresOnDeviceRecognition = true
    request.shouldReportPartialResults = false
    request.addsPunctuation = true
    request.taskHint = .dictation
    let box = SpeechResult()
    let task = recognizer.recognitionTask(with: request) { result, error in
        if let result, result.isFinal {
            box.words = result.bestTranscription.segments.map {
                ["t": $0.substring, "s": $0.timestamp, "d": $0.duration]
            }
            box.finished = true
        } else if let error {
            box.error = error
            box.finished = true
        }
    }
    let deadline = Date().addingTimeInterval(300)
    while !box.finished && Date() < deadline {
        RunLoop.main.run(mode: .default, before: Date().addingTimeInterval(0.1))
    }
    guard box.finished else {
        task.cancel()
        throw Failure.failed("speech recognition took too long")
    }
    if let error = box.error {
        // A chunk with only music or silence isn't a failure.
        let ns = error as NSError
        if ns.code == 1110 || ns.localizedDescription.lowercased().contains("no speech") { return [] }
        throw Failure.failed("speech recognition failed: \(error.localizedDescription)")
    }
    return box.words
}

func transcribe(_ args: [String]) throws {
    let tag = args[0]
    let output = URL(fileURLWithPath: args[1])
    let chunks = args.dropFirst(2).map { URL(fileURLWithPath: $0) }
    try requireSpeechPermission()
    let recognizer = try speechRecognizer(tag)
    var all: [[[String: Any]]] = []
    for (i, chunk) in chunks.enumerated() {
        all.append(try recognizeSpeech(chunk, with: recognizer))
        report(Double(i + 1) / Double(max(chunks.count, 1)))
    }
    if all.allSatisfy({ $0.isEmpty }) { throw Failure.nothingFound("no speech found") }
    let answer: [String: Any] = ["locale": recognizer.locale.identifier, "chunks": all]
    let data = try JSONSerialization.data(withJSONObject: answer)
    try data.write(to: output)
}

// MARK: - Self-test

func selftest() throws {
    let dir = FileManager.default.temporaryDirectory.appendingPathComponent("veer-vision-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }

    // A white picture with a line of black text.
    let w = 1400, h = 320
    guard let space = CGColorSpace(name: CGColorSpace.sRGB),
          let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0, space: space,
                              bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else {
        throw Failure.failed("couldn't draw the test picture")
    }
    ctx.setFillColor(CGColor(gray: 1, alpha: 1))
    ctx.fill(CGRect(x: 0, y: 0, width: w, height: h))
    let font = CTFontCreateWithName("Helvetica" as CFString, 110, nil)
    let attributed = NSAttributedString(string: "Veer reads 2026", attributes: [
        NSAttributedString.Key(kCTFontAttributeName as String): font,
        NSAttributedString.Key(kCTForegroundColorAttributeName as String): CGColor(gray: 0, alpha: 1),
    ])
    ctx.textPosition = CGPoint(x: 60, y: 110)
    CTLineDraw(CTLineCreateWithAttributedString(attributed as CFAttributedString), ctx)
    guard let picture = ctx.makeImage(),
          let dest = CGImageDestinationCreateWithURL(dir.appendingPathComponent("test.png") as CFURL,
                                                     UTType.png.identifier as CFString, 1, nil) else {
        throw Failure.failed("couldn't save the test picture")
    }
    CGImageDestinationAddImage(dest, picture, nil)
    CGImageDestinationFinalize(dest)
    let png = dir.appendingPathComponent("test.png")

    let txt = dir.appendingPathComponent("test.txt")
    try text(png, txt)
    let read = try String(contentsOf: txt, encoding: .utf8)
    guard read.contains("2026") else { throw Failure.failed("text recognition read \"\(read)\"") }
    print("text: ok (\(read.trimmingCharacters(in: .whitespacesAndNewlines)))")

    let pdf = dir.appendingPathComponent("test.pdf")
    try searchable(png, pdf)
    guard let found = PDFDocument(url: pdf)?.string, found.contains("2026") else {
        throw Failure.failed("the searchable PDF has no text")
    }
    print("searchable: ok")

    // Vision may not call a plain test picture a "subject", and some
    // virtual Macs can't run the model; neither fails the self-test.
    do {
        try mask(png, dir.appendingPathComponent("mask.png"))
        print("mask: ok")
    } catch Failure.nothingFound {
        print("mask: ran (no subject in the test picture)")
    } catch {
        print("mask: unavailable here (\(error.localizedDescription))")
    }

    // Speech needs permission and a downloaded language, so it's reported
    // rather than required.
    do {
        let recognizer = try speechRecognizer("auto")
        let allowed = SFSpeechRecognizer.authorizationStatus() == .authorized
        print("speech: on-device \(recognizer.locale.identifier)"
              + (allowed ? "" : " (allow it once with Make Subtitles in Veer)"))
    } catch Failure.failed(let message) {
        print("speech: unavailable here (\(message))")
    }
}

// MARK: - Main

let args = Array(CommandLine.arguments.dropFirst())
do {
    switch args.first {
    case "text" where args.count == 3:
        try text(URL(fileURLWithPath: args[1]), URL(fileURLWithPath: args[2]))
    case "mask" where args.count == 3:
        try mask(URL(fileURLWithPath: args[1]), URL(fileURLWithPath: args[2]))
    case "searchable" where args.count == 3:
        try searchable(URL(fileURLWithPath: args[1]), URL(fileURLWithPath: args[2]))
    case "transcribe" where args.count >= 4:
        try transcribe(Array(args.dropFirst()))
    case "document-pdf" where args.count == 3:
        try documentPDF(URL(fileURLWithPath: args[1]), URL(fileURLWithPath: args[2]))
    case "selftest":
        try selftest()
    case "--version":
        print("veer-vision 1")
    default:
        FileHandle.standardError.write(Data("usage: veer-vision text|mask|searchable|document-pdf <input> <output>, transcribe <language> <output.json> <chunk>..., or selftest\n".utf8))
        exit(2)
    }
} catch Failure.nothingFound(let message) {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(3)
} catch Failure.failed(let message) {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(1)
} catch {
    FileHandle.standardError.write(Data((error.localizedDescription + "\n").utf8))
    exit(1)
}
