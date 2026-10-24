// Translating text on this Mac with Apple's Translation framework
// (macOS 26 and later). The engine sends a JSON list of strings (the
// paragraphs, headings, cells or subtitle lines of one file) and gets the
// same list back, translated. Nothing leaves the Mac; languages are
// downloaded once in System Settings.
//
//   radlet-vision translate <to> <from|auto> <input.json> <output.json>
//
// Exit 3 (nothing to do) when the text is already in the target language
// or its language can't be told.

import Foundation
import NaturalLanguage
#if compiler(>=6.2)
// Weakly linked: the helper still starts on Macs without the framework.
@_weakLinked import Translation
#endif

/// The result of async work, collected on the main run loop.
final class AsyncBox<T> {
    var result: Result<T, Error>?
}

/// Runs async work from this command-line program and waits for it,
/// servicing the main run loop meanwhile (the frameworks may need it).
func blocking<T>(_ work: @escaping () async throws -> T) throws -> T {
    let box = AsyncBox<T>()
    Task.detached {
        let result: Result<T, Error>
        do { result = .success(try await work()) } catch { result = .failure(error) }
        DispatchQueue.main.async { box.result = result }
    }
    while box.result == nil {
        RunLoop.main.run(mode: .default, before: Date().addingTimeInterval(0.05))
    }
    return try box.result!.get()
}

/// The language most of the text is in, as a tag ("en", "zh-Hans").
func detectLanguage(_ texts: [String]) -> String? {
    var sample = ""
    for t in texts where !t.isEmpty {
        sample += t + "\n"
        if sample.count > 6000 { break }
    }
    let recognizer = NLLanguageRecognizer()
    recognizer.processString(sample)
    return recognizer.dominantLanguage?.rawValue
}

func languageName(_ tag: String) -> String {
    Locale.current.localizedString(forIdentifier: tag) ?? tag
}

/// Whether two tags name the same language and script ("en" and "en-GB"
/// do; "zh-Hans" and "zh-Hant" don't).
func sameLanguage(_ a: String, _ b: String) -> Bool {
    let x = Locale.Language(identifier: Locale.Language(identifier: a).maximalIdentifier)
    let y = Locale.Language(identifier: Locale.Language(identifier: b).maximalIdentifier)
    return x.languageCode == y.languageCode && x.script == y.script
}

func translateTexts(_ args: [String]) throws {
    let to = args[0], from = args[1]
    let input = URL(fileURLWithPath: args[2]), output = URL(fileURLWithPath: args[3])
    guard let texts = try JSONSerialization.jsonObject(with: Data(contentsOf: input)) as? [String] else {
        throw Failure.failed("the text to translate doesn't read")
    }
    let source: String
    if from == "auto" {
        guard let found = detectLanguage(texts) else {
            throw Failure.nothingFound("couldn't tell what language this is in")
        }
        source = found
    } else {
        source = from
    }
    if sameLanguage(source, to) {
        throw Failure.nothingFound("it's already in \(languageName(to))")
    }
    #if compiler(>=6.2)
    if #available(macOS 26.0, *) {
        let done = try translate(texts, from: source, to: to)
        try JSONSerialization.data(withJSONObject: done).write(to: output)
        return
    }
    #endif
    throw Failure.failed("Translating files needs macOS 26 or later.")
}

#if compiler(>=6.2)
@available(macOS 26.0, *)
func translate(_ texts: [String], from: String, to: String) throws -> [String] {
    let source = Locale.Language(identifier: from), target = Locale.Language(identifier: to)
    let names = (languageName(from), languageName(to))
    let status = try blocking { await LanguageAvailability().status(from: source, to: target) }
    switch status {
    case .installed:
        break
    case .supported:
        throw Failure.failed("Download \(names.0) and \(names.1) in System Settings → General → "
                             + "Language & Region → Translation Languages, then try again.")
    case .unsupported:
        throw Failure.failed("This Mac can't translate \(names.0) into \(names.1).")
    @unknown default:
        break
    }
    let session = TranslationSession(installedSource: source, target: target)
    var out = texts
    // Blank strings stay as they are; the rest go in batches, for progress.
    let todo = texts.indices.filter { !texts[$0].trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    var finished = 0
    for start in stride(from: 0, to: todo.count, by: 48) {
        let batch = Array(todo[start..<min(start + 48, todo.count)])
        let requests = batch.map { TranslationSession.Request(sourceText: texts[$0], clientIdentifier: String($0)) }
        let responses: [TranslationSession.Response]
        do {
            responses = try blocking { try await session.translations(from: requests) }
        } catch {
            throw Failure.failed("translation failed: \(error.localizedDescription)")
        }
        guard responses.count == batch.count else { throw Failure.failed("the translation came back incomplete") }
        for (i, response) in zip(batch, responses) { out[i] = response.targetText }
        finished += batch.count
        report(Double(finished) / Double(max(todo.count, 1)))
    }
    return out
}
#endif

/// One line for the self-test.
func translationStatus() -> String {
    #if compiler(>=6.2)
    if #available(macOS 26.0, *) {
        return "translate: available (languages download in System Settings → General → Language & Region)"
    }
    return "translate: needs macOS 26"
    #else
    return "translate: not in this build (needs the macOS 26 SDK)"
    #endif
}
