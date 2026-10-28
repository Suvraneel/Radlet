// Words for what's in a file, made on this Mac: what Vision sees in a
// picture (its labels and any text), and Apple Intelligence's on-device
// language model (macOS 26 on Apple silicon) to write titles, alt text and
// summaries. Nothing leaves the Mac.
//
//   radlet-vision describe title|alt <image> <output.json>
//       {"title": …, "description": …}. A title works on every Mac: the
//       model writes it when Apple Intelligence is on, otherwise it comes
//       from the text in the picture or what Vision recognises. Alt text
//       (the description) needs Apple Intelligence.
//   radlet-vision write title|summary <input.txt> <output.txt>
//       a document's title, or a summary of it, by the model.
//
// Without Apple Intelligence where it's needed: exit 4 with "need
// apple_intelligence" (see main.swift). The model can't see pictures, so
// what it writes about one comes from Vision's labels and text.

import Foundation
import Vision
#if compiler(>=6.2)
// Weakly linked: the helper still starts on Macs without the framework.
@_weakLinked import FoundationModels
#endif

/// What Vision recognises in a picture.
struct Seen {
    /// Most confident first, such as "dog", "beach".
    var labels: [String]
    /// Text in the picture, line by line.
    var text: String
}

/// Labels too broad to say anything on their own.
let broadLabels: Set<String> = [
    "structure", "outdoor", "indoor", "material", "animal", "mammal", "people", "adult", "art",
    "document", "liquid", "water body", "land", "plant", "vegetation", "consumer electronics",
    "machine", "textile", "wood processed", "container", "object",
]

func see(_ input: URL) throws -> Seen {
    let file = try ImageFile(input)
    guard let image = file.upright(0, longest: 1600) else { throw Failure.failed("couldn't read this image") }
    let classify = VNClassifyImageRequest()
    let read = VNRecognizeTextRequest()
    read.recognitionLevel = .accurate
    read.usesLanguageCorrection = true
    read.automaticallyDetectsLanguage = true
    try VNImageRequestHandler(cgImage: image, options: [:]).perform([classify, read])
    let labels = (classify.results ?? [])
        .filter { $0.confidence >= 0.3 }
        .sorted { $0.confidence > $1.confidence }
        .prefix(10)
        .map { $0.identifier.replacingOccurrences(of: "_", with: " ") }
    return Seen(labels: Array(labels), text: lines(read.results ?? []))
}

/// A title without the model: a line of text in the picture if it has
/// one worth using (a screenshot, a sign, a receipt), else its most
/// specific labels.
func plainTitle(_ seen: Seen) -> String? {
    let wordy = seen.text.split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }.first { line in
        let words = line.split(separator: " ").count
        return words >= 2 && words <= 10 && line.filter(\.isLetter).count >= 6
    }
    if let wordy { return wordy }
    let specific = seen.labels.filter { !broadLabels.contains($0) }
    let picked = (specific.isEmpty ? seen.labels : specific).prefix(3)
    return picked.isEmpty ? nil : picked.joined(separator: " ")
}

/// A prompt's description of a picture: its labels and text.
func promptFacts(_ seen: Seen) -> String {
    var facts = "Things Vision recognised in it, most certain first: "
        + (seen.labels.isEmpty ? "(none)" : seen.labels.joined(separator: ", ")) + "."
    let text = seen.text.trimmingCharacters(in: .whitespacesAndNewlines)
    if !text.isEmpty { facts += "\nText in it:\n" + String(text.prefix(800)) }
    return facts
}

func describePicture(_ args: [String]) throws {
    let alt = args[0] == "alt"
    guard alt || args[0] == "title" else { throw Failure.failed("describe takes title or alt") }
    let seen = try see(URL(fileURLWithPath: args[1]))
    let output = URL(fileURLWithPath: args[2])
    if seen.labels.isEmpty && seen.text.isEmpty { throw Failure.nothingFound("nothing recognisable in this picture") }
    var result: [String: String] = [:]
    let facts = promptFacts(seen)
    let titled = try ask(
        "You name picture files. Answer with only a file name of 2 to 6 words, in sentence case, with no quotes, punctuation or file extension.",
        "Name this picture.\n\(facts)",
        required: false)
    result["title"] = titled ?? plainTitle(seen) ?? ""
    report(0.5)
    if alt {
        result["description"] = try ask(
            "You write alt text for pictures, for people who can't see them. Use only the facts given; never invent names, places or details. Answer with one or two plain sentences, and no preamble.",
            "Write alt text for this picture.\n\(facts)",
            required: true)
    }
    try JSONSerialization.data(withJSONObject: result).write(to: output)
}

func writeAbout(_ args: [String]) throws {
    let task = args[0]
    let input = URL(fileURLWithPath: args[1])
    let output = URL(fileURLWithPath: args[2])
    let text = try String(contentsOf: input, encoding: .utf8)
    let answer: String
    switch task {
    case "title":
        answer = try ask(
            "You name document files. Answer with only a specific file name of 2 to 7 words, in sentence case, with no quotes, punctuation or file extension.",
            "Name this document:\n\n\(text.prefix(6000))",
            required: true) ?? ""
    case "summary":
        answer = try summarise(text)
    default:
        throw Failure.failed("write takes title or summary")
    }
    guard !answer.isEmpty else { throw Failure.nothingFound("nothing came back") }
    try (answer + "\n").write(to: output, atomically: false, encoding: .utf8)
}

/// Splits text into parts of about `size` characters, at paragraph ends
/// where it can.
func parts(_ text: String, size: Int) -> [String] {
    var out: [String] = []
    var current = ""
    for paragraph in text.components(separatedBy: "\n") {
        if current.count + paragraph.count > size, !current.isEmpty {
            out.append(current)
            current = ""
        }
        if paragraph.count > size {
            var rest = Substring(paragraph)
            while rest.count > size {
                out.append(String(rest.prefix(size)))
                rest = rest.dropFirst(size)
            }
            current = String(rest)
        } else {
            current += paragraph + "\n"
        }
    }
    if !current.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { out.append(current) }
    return out
}

/// A summary in Markdown: each part summarised, then the notes combined.
func summarise(_ text: String) throws -> String {
    let instructions = "You summarise documents accurately. Keep names, numbers and dates exactly as written. Never add facts. Answer in Markdown bullet points, with no preamble."
    let pieces = Array(parts(text, size: 6000).prefix(8))
    if pieces.count <= 1 {
        report(0.2)
        return try ask(instructions,
                       "Summarise this document in 3 to 8 bullet points:\n\n\(text.prefix(6000))",
                       required: true) ?? ""
    }
    var notes: [String] = []
    for (i, piece) in pieces.enumerated() {
        let note = try ask(instructions,
                           "Summarise part \(i + 1) of \(pieces.count) of a document in up to 5 bullet points:\n\n\(piece)",
                           required: true) ?? ""
        notes.append(note)
        report(Double(i + 1) / Double(pieces.count + 1))
    }
    return try ask(
        instructions,
        "These are notes on the parts of one document, in order. Combine them into one summary: a one-sentence overview, then up to 8 bullet points.\n\n" + notes.joined(separator: "\n\n"),
        required: true) ?? ""
}

/// The on-device model's answer, or nil when this Mac has no model and
/// `required` is false. Without one where it's required: exit 4.
func ask(_ instructions: String, _ prompt: String, required: Bool) throws -> String? {
    #if compiler(>=6.2)
    if #available(macOS 26.0, *) {
        let model = SystemLanguageModel.default
        switch model.availability {
        case .available:
            let session = LanguageModelSession(instructions: instructions)
            do {
                let answer = try blocking { try await session.respond(to: prompt).content }
                return answer.trimmingCharacters(in: .whitespacesAndNewlines)
            } catch {
                if !required { return nil }
                throw Failure.failed("Apple Intelligence couldn't write this (\(error.localizedDescription))")
            }
        case .unavailable(let reason):
            guard required else { return nil }
            switch reason {
            case .deviceNotEligible:
                throw Failure.failed("this Mac can't run Apple Intelligence")
            case .appleIntelligenceNotEnabled:
                throw Failure.needs("apple_intelligence",
                                    "turn on Apple Intelligence in System Settings → Apple Intelligence & Siri")
            case .modelNotReady:
                throw Failure.needs("apple_intelligence",
                                    "Apple Intelligence is still getting ready on this Mac; try again in a while")
            @unknown default:
                throw Failure.failed("Apple Intelligence isn't available on this Mac")
            }
        }
    }
    #endif
    guard required else { return nil }
    throw Failure.needs("apple_intelligence", "this needs Apple Intelligence, on macOS 26 or later with Apple silicon")
}

/// For selftest: whether the on-device model can be used here.
func intelligenceStatus() -> String {
    #if compiler(>=6.2)
    if #available(macOS 26.0, *) {
        switch SystemLanguageModel.default.availability {
        case .available: return "on"
        case .unavailable(.appleIntelligenceNotEnabled): return "turned off (System Settings → Apple Intelligence & Siri)"
        case .unavailable(.modelNotReady): return "getting ready"
        case .unavailable(.deviceNotEligible): return "not on this Mac"
        case .unavailable: return "unavailable"
        }
    }
    #endif
    return "needs macOS 26"
}
