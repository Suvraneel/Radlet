// Barcodes and QR codes in a picture, with Vision: QR, Aztec, PDF417,
// Data Matrix, EAN, UPC, Code 128, Code 39, ITF and the rest Vision knows.
//
//   radlet-vision codes <input> <output.json>
//
// Writes [{"kind": "EAN-13", "value": "4006381333931"}, ...], top to bottom,
// each value once. Exit 3 when there's none.

import Foundation
import Vision

func codes(_ input: URL, _ output: URL) throws {
    let (image, orientation) = try ImageFile(input).page(0)
    let request = VNDetectBarcodesRequest()
    try VNImageRequestHandler(cgImage: image, orientation: orientation, options: [:]).perform([request])
    // Vision's boxes have their origin at the bottom left: top first.
    let found = (request.results ?? []).sorted { $0.boundingBox.maxY > $1.boundingBox.maxY }
    var seen = Set<String>()
    var answer: [[String: String]] = []
    for observation in found {
        guard let value = observation.payloadStringValue, !value.isEmpty, seen.insert(value).inserted else { continue }
        answer.append(["kind": symbologyName(observation.symbology), "value": value])
    }
    if answer.isEmpty { throw Failure.nothingFound("no QR code or barcode found") }
    try JSONSerialization.data(withJSONObject: answer).write(to: output)
}

func symbologyName(_ symbology: VNBarcodeSymbology) -> String {
    switch symbology {
    case .qr: return "QR code"
    case .microQR: return "Micro QR code"
    case .aztec: return "Aztec"
    case .pdf417: return "PDF417"
    case .microPDF417: return "MicroPDF417"
    case .dataMatrix: return "Data Matrix"
    case .ean8: return "EAN-8"
    case .ean13: return "EAN-13"
    case .upce: return "UPC-E"
    case .code128: return "Code 128"
    case .code39, .code39Checksum, .code39FullASCII, .code39FullASCIIChecksum: return "Code 39"
    case .code93, .code93i: return "Code 93"
    case .itf14, .i2of5, .i2of5Checksum: return "ITF"
    case .codabar: return "Codabar"
    case .gs1DataBar, .gs1DataBarExpanded, .gs1DataBarLimited: return "GS1 DataBar"
    default: return "Barcode"
    }
}
