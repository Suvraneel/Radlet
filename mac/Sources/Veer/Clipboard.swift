import AppKit
import UniformTypeIdentifiers
import VeerKit

/// "Convert Clipboard": files copied in Finder go to the wheel as they
/// are; a copied image (a screenshot, say) is saved as a PNG first.
/// Results go back on the clipboard, ready to paste.
@MainActor
enum ClipboardInput {
    /// Where copied images are saved for converting. Results never go
    /// here: see `isTemporary`.
    static var folder: URL {
        let base = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent("Library/Caches")
        return base.appendingPathComponent("Veer/Clipboard", isDirectory: true)
    }

    /// Files to convert, or nil when the clipboard holds neither files nor
    /// an image.
    static func read() -> [URL]? {
        let pb = NSPasteboard.general
        if let urls = pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL],
           !urls.isEmpty {
            // Folders count too (zip, merge, list…). Finder also puts a
            // copied folder's icon on the clipboard; the folder wins.
            let files = urls.filter { FileManager.default.fileExists(atPath: $0.path) }
            return files.isEmpty ? nil : files
        }
        return saveImage(from: pb).map { [$0] }
    }

    private static func pngData(from pb: NSPasteboard) -> Data? {
        if let png = pb.data(forType: .png) { return png }
        let tiff = pb.data(forType: .tiff) ?? NSImage(pasteboard: pb)?.tiffRepresentation
        guard let tiff, let rep = NSBitmapImageRep(data: tiff) else { return nil }
        return rep.representation(using: .png, properties: [:])
    }

    private static func saveImage(from pb: NSPasteboard) -> URL? {
        guard let data = pngData(from: pb) else { return nil }
        let fm = FileManager.default
        do {
            try fm.createDirectory(at: folder, withIntermediateDirectories: true)
        } catch {
            Log.app("clipboard: couldn't make the folder: \(Log.code(error))")
            return nil
        }
        let stamp = DateFormatter()
        stamp.locale = Locale(identifier: "en_US_POSIX")
        stamp.dateFormat = "yyyy-MM-dd 'at' HH.mm.ss"
        let base = "Clipboard \(stamp.string(from: Date()))"
        for n in 1..<100 {
            let url = folder.appendingPathComponent(n == 1 ? "\(base).png" : "\(base) \(n).png")
            do {
                try data.write(to: url, options: .withoutOverwriting)
                Log.app("clipboard: saved a copied image (\(data.count) bytes)")
                return url
            } catch CocoaError.fileWriteFileExists {
                continue
            } catch {
                Log.app("clipboard: couldn't save the image: \(Log.code(error))")
                return nil
            }
        }
        return nil
    }

    /// True for files saved from the clipboard, whose results must not be
    /// saved beside them (nobody would find them in Caches).
    static func isTemporary(_ files: [URL]) -> Bool {
        let base = folder.standardizedFileURL.path + "/"
        return !files.isEmpty && files.allSatisfy { $0.standardizedFileURL.path.hasPrefix(base) }
    }

    /// Clears saved images from earlier days.
    static func removeOld() {
        let fm = FileManager.default
        guard let items = try? fm.contentsOfDirectory(at: folder, includingPropertiesForKeys: [.contentModificationDateKey])
        else { return }
        let cutoff = Date().addingTimeInterval(-24 * 3600)
        for url in items {
            let date = (try? url.resourceValues(forKeys: [.contentModificationDateKey]))?.contentModificationDate
            if let date, date < cutoff { try? fm.removeItem(at: url) }
        }
    }

    /// Puts results on the clipboard: as files, and a single image also as
    /// a picture, so it pastes into Finder, Mail and chat apps alike.
    static func copy(_ outputs: [URL]) {
        let files = outputs.filter { FileManager.default.fileExists(atPath: $0.path) }
        guard !files.isEmpty else { return }
        let pb = NSPasteboard.general
        pb.clearContents()
        if files.count == 1, let type = UTType(filenameExtension: files[0].pathExtension), type.conforms(to: .image),
           let data = try? Data(contentsOf: files[0]), data.count < 64 << 20 {
            let item = NSPasteboardItem()
            item.setString(files[0].absoluteString, forType: .fileURL)
            item.setData(data, forType: NSPasteboard.PasteboardType(type.identifier))
            pb.writeObjects([item])
        } else {
            pb.writeObjects(files.map { $0 as NSURL })
        }
    }
}
