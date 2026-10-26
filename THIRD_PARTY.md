# Third-party software

Radlet's own code is original. It depends on the following open-source components, all under permissive licences. `cargo deny check licenses` (see `deny.toml`) fails the build if a dependency with any other licence is added.

## Rust crates (engine and CLI)

| Crate | Used for | Licence |
| --- | --- | --- |
| image | Decoding and encoding PNG, JPEG, GIF, BMP, TIFF, ICO, TGA, QOI, lossless WebP | MIT OR Apache-2.0 |
| webp / libwebp-sys (bundles Google libwebp) | Lossy WebP encoding | MIT OR Apache-2.0; libwebp BSD-3-Clause |
| lopdf | Building, merging and cleaning PDFs; text extraction | MIT |
| flate2 / miniz_oxide | PDF stream compression | MIT OR Apache-2.0 |
| pulldown-cmark | Reading Markdown | MIT |
| sha2 | SHA-256 checksums | MIT OR Apache-2.0 |
| serde, serde_json, toml | Protocol and recipe formats; JSON and TOML files | MIT OR Apache-2.0 |
| clap | Command-line parsing | MIT OR Apache-2.0 |
| tempfile | Temporary files | MIT OR Apache-2.0 |
| getrandom | Random keys for PDF passwords | MIT OR Apache-2.0 |
| kamadak-exif | Reading EXIF (location, camera, dates) for the Privacy check | BSD-2-Clause |
| thiserror, anyhow | Errors | MIT OR Apache-2.0 |
| libc | Process-group signalling on macOS and Linux; local time for archive dates | MIT OR Apache-2.0 |
| resvg / usvg / tiny-skia (with fontdb and rustybuzz for text) | Drawing SVGs into pixels | Apache-2.0 OR MIT; tiny-skia BSD-3-Clause |
| svg2pdf / pdf-writer | SVG to vector PDF | MIT OR Apache-2.0 |
| zip | Making and extracting ZIP archives; reading Word files | MIT |
| quick-xml | Reading Word documents, XML and TTML subtitles | MIT |
| csv | Reading and writing CSV and TSV | Unlicense OR MIT |
| calamine | Reading Excel workbooks | MIT |
| rust_xlsxwriter | Writing Excel workbooks | MIT OR Apache-2.0 |
| rqrr | Reading QR codes (its decoder includes code under the ISC licence) | (MIT OR Apache-2.0) AND ISC |
| qrcode | Making QR codes | MIT OR Apache-2.0 |
| yaml-rust2 | Reading and writing YAML | MIT OR Apache-2.0 |

Transitive dependencies are also permissive: MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib, 0BSD, Unlicense, Unicode-3.0. Run `cargo deny list` for the full set.

A shipped app should include these notices in an About → Acknowledgements screen. That's tracked on the roadmap.

## Bundled libraries

- **PDFium** (BSD-3-Clause), the PDF renderer from Chromium, used for PDF to image. The app bundles a prebuilt copy from the [pdfium-binaries](https://github.com/bblanchon/pdfium-binaries) project (packaging under MIT) and loads it at run time; it isn't linked into the helper. PDFium includes further permissively licensed libraries: FreeType, HarfBuzz, ICU, Abseil, libjpeg-turbo, libpng, zlib, OpenJPEG, Little CMS, Highway and a few smaller ones. `build-app.sh` copies every licence file from the PDFium download into `Radlet.app/Contents/Resources/Licenses/PDFium/`.

- **Sparkle** (MIT), the update framework, installs new releases from Radlet's appcast after checking their EdDSA signature. The app bundles `Sparkle.framework` (from Sparkle's Swift package, pinned in `mac/Package.swift`) without its XPC services, which only sandboxed apps need. Sparkle includes bsdiff and SUSignatureVerifier (BSD-2-Clause), sais-lite (MIT) and ed25519 (zlib), as listed in its licence file. `build-app.sh` copies Sparkle's licence into `Radlet.app/Contents/Resources/Licenses/Sparkle.txt`.

## External programs

- **FFmpeg / FFprobe.** Optional. Radlet runs them as separate programs and doesn't link them. If a release bundles them, it must be an **LGPL** build (no `--enable-gpl` or `--enable-nonfree`), shipped with its licence and a link to its source.
- **sips.** Part of macOS. Used for HEIC, AVIF, camera RAW, PSD, JPEG XL and JPEG 2000.
- **textutil.** Part of macOS. Used to read Word 97 (.doc) and OpenDocument (.odt) files.
- **tar (bsdtar).** Part of macOS (libarchive, BSD licence). Used for 7-Zip, RAR, TAR and ISO archives, and to pack .tar.gz.

## Apple frameworks

AppKit, SwiftUI, ServiceManagement, UniformTypeIdentifiers, PDFKit, AVFoundation, Core Image, Core Text, Vision, Speech and Translation are part of macOS and aren't redistributed. Text recognition, barcodes and subject masks run on-device through Vision, speech recognition for subtitles through Speech, and audio and video conversion without FFmpeg through AVFoundation.
