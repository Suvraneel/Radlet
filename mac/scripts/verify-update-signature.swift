// Checks that an update archive's EdDSA signature matches the public key in
// Radlet's Info.plist, the way Sparkle will check it before installing.
// The release workflow runs this so a mismatched key can never ship.
//
//   swift mac/scripts/verify-update-signature.swift <public-key> <signature> <archive>
import CryptoKit
import Foundation

let arguments = CommandLine.arguments
guard arguments.count == 4 else {
    FileHandle.standardError.write(Data("usage: verify-update-signature.swift <public-key> <signature> <archive>\n".utf8))
    exit(2)
}
guard let keyData = Data(base64Encoded: arguments[1]),
      let key = try? Curve25519.Signing.PublicKey(rawRepresentation: keyData) else {
    FileHandle.standardError.write(Data("The public key isn't a base64 Ed25519 key.\n".utf8))
    exit(1)
}
guard let signature = Data(base64Encoded: arguments[2]) else {
    FileHandle.standardError.write(Data("The signature isn't base64.\n".utf8))
    exit(1)
}
guard let archive = FileManager.default.contents(atPath: arguments[3]) else {
    FileHandle.standardError.write(Data("Couldn't read \(arguments[3]).\n".utf8))
    exit(1)
}
if key.isValidSignature(signature, for: archive) {
    print("The update's signature matches the app's public key.")
} else {
    FileHandle.standardError.write(Data("The update's signature doesn't match SUPublicEDKey: installed copies would refuse it.\n".utf8))
    exit(1)
}
