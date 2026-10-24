import Foundation

// Skins restyle the wheel. A skin is a small CSS file of custom
// properties, so anyone who has written CSS can make one:
//
//     :root {
//       --base: neon;
//       --accent: #00FF9C;
//     }
//
// Anything a skin leaves out comes from its --base (Glass, the plain system
// look, if none).
// Skins only hold colours, sizes, keywords and font names: no paths, URLs
// or code, so a skin from anywhere is safe to drop in.

/// A colour: sRGB components from 0 to 1, or one of the Mac's own colours,
/// which follow light and dark mode and the system accent.
public enum SkinColor: Equatable, Sendable {
    case rgba(Double, Double, Double, Double)
    case system(SystemColor)

    public enum SystemColor: String, CaseIterable, Sendable {
        case accent = "system-accent"
        case label
        case secondaryLabel = "secondary-label"
        case tertiaryLabel = "tertiary-label"
        case separator
        case window
        case control
    }

    public init(hex: UInt32, alpha: Double = 1) {
        self = .rgba(Double((hex >> 16) & 0xFF) / 255, Double((hex >> 8) & 0xFF) / 255, Double(hex & 0xFF) / 255, alpha)
    }

    /// Reads a CSS colour: #rgb, #rgba, #rrggbb, #rrggbbaa, rgb(), rgba(),
    /// hsl(), hsla(), transparent, black, white, or a Mac colour name.
    public init?(css raw: String) {
        let s = raw.trimmingCharacters(in: .whitespaces).lowercased()
        if let system = SystemColor(rawValue: s) {
            self = .system(system)
            return
        }
        switch s {
        case "transparent": self = .rgba(0, 0, 0, 0); return
        case "black": self = .rgba(0, 0, 0, 1); return
        case "white": self = .rgba(1, 1, 1, 1); return
        default: break
        }
        if s.hasPrefix("#") {
            guard let c = SkinColor.hex(String(s.dropFirst())) else { return nil }
            self = c
            return
        }
        guard let open = s.firstIndex(of: "("), s.hasSuffix(")") else { return nil }
        let fn = String(s[..<open]).trimmingCharacters(in: .whitespaces)
        let inside = String(s[s.index(after: open)..<s.index(before: s.endIndex)])
        // Commas, spaces, or the modern "r g b / a".
        let parts = inside.replacingOccurrences(of: "/", with: " ").replacingOccurrences(of: ",", with: " ")
            .split(separator: " ").map(String.init)
        guard parts.count == 3 || parts.count == 4 else { return nil }
        let alpha: Double
        if parts.count == 4 {
            guard let a = SkinColor.component(parts[3], scale: 1) else { return nil }
            alpha = a
        } else {
            alpha = 1
        }
        switch fn {
        case "rgb", "rgba":
            guard let r = SkinColor.component(parts[0], scale: 255),
                  let g = SkinColor.component(parts[1], scale: 255),
                  let b = SkinColor.component(parts[2], scale: 255) else { return nil }
            self = .rgba(r, g, b, alpha)
        case "hsl", "hsla":
            guard let h = Double(parts[0].replacingOccurrences(of: "deg", with: "")), h.isFinite,
                  parts[1].hasSuffix("%"), parts[2].hasSuffix("%"),
                  let sat = Double(parts[1].dropLast()), let light = Double(parts[2].dropLast()),
                  sat.isFinite, light.isFinite else { return nil }
            let (r, g, b) = SkinColor.hslToRGB(h, min(max(sat, 0), 100) / 100, min(max(light, 0), 100) / 100)
            self = .rgba(r, g, b, alpha)
        default:
            return nil
        }
    }

    private static func hex(_ h: String) -> SkinColor? {
        guard [3, 4, 6, 8].contains(h.count), h.allSatisfy(\.isHexDigit) else { return nil }
        let full = h.count <= 4 ? h.map { "\($0)\($0)" }.joined() : h
        guard let v = UInt64(full, radix: 16) else { return nil }
        if full.count == 8 {
            return .rgba(Double((v >> 24) & 0xFF) / 255, Double((v >> 16) & 0xFF) / 255,
                         Double((v >> 8) & 0xFF) / 255, Double(v & 0xFF) / 255)
        }
        return SkinColor(hex: UInt32(v))
    }

    /// A number, or a percentage of `scale`, as 0...1.
    private static func component(_ s: String, scale: Double) -> Double? {
        if s.hasSuffix("%") {
            guard let p = Double(s.dropLast()), p.isFinite else { return nil }
            return min(max(p / 100, 0), 1)
        }
        guard let v = Double(s), v.isFinite else { return nil }
        return min(max(v / scale, 0), 1)
    }

    private static func hslToRGB(_ hue: Double, _ s: Double, _ l: Double) -> (Double, Double, Double) {
        var h = hue.truncatingRemainder(dividingBy: 360)
        if h < 0 { h += 360 }
        let c = (1 - abs(2 * l - 1)) * s
        let x = c * (1 - abs((h / 60).truncatingRemainder(dividingBy: 2) - 1))
        let m = l - c / 2
        let (r, g, b): (Double, Double, Double)
        switch h {
        case ..<60: (r, g, b) = (c, x, 0)
        case ..<120: (r, g, b) = (x, c, 0)
        case ..<180: (r, g, b) = (0, c, x)
        case ..<240: (r, g, b) = (0, x, c)
        case ..<300: (r, g, b) = (x, 0, c)
        default: (r, g, b) = (c, 0, x)
        }
        return (r + m, g + m, b + m)
    }

    /// How a skin file writes it.
    public var css: String {
        switch self {
        case .system(let s):
            return s.rawValue
        case let .rgba(r, g, b, a):
            let byte = { (v: Double) -> Int in v.isFinite ? Int((min(max(v, 0), 1) * 255).rounded()) : 0 }
            let ints = [r, g, b].map(byte)
            let alpha = a.isFinite ? min(max(a, 0), 1) : 1
            if alpha == 1 { return String(format: "#%02X%02X%02X", ints[0], ints[1], ints[2]) }
            // An alpha that came from hex (n/255) is written as hex, so it reads back exactly.
            let a255 = alpha * 255
            if abs(a255 - a255.rounded()) < 1e-9, Double(SkinNumber.format(alpha)) != alpha {
                return String(format: "#%02X%02X%02X%02X", ints[0], ints[1], ints[2], Int(a255.rounded()))
            }
            return "rgba(\(ints[0]), \(ints[1]), \(ints[2]), \(SkinNumber.format(alpha)))"
        }
    }
}

/// A typeface: the system's, in one of its designs, or one installed on
/// the Mac by family name. A missing font falls back to the system's.
public enum SkinFont: Equatable, Sendable {
    case system(Design)
    case named(String)

    public enum Design: String, CaseIterable, Sendable {
        case standard = "system"
        case rounded = "system-rounded"
        case serif = "system-serif"
        case mono = "system-mono"
    }

    public var css: String {
        switch self {
        case .system(let d): return d.rawValue
        case .named(let n):
            if !n.contains("\"") { return "\"\(n)\"" }
            return n.contains("'") ? "\"\(n.replacingOccurrences(of: "\"", with: ""))\"" : "'\(n)'"
        }
    }
}

/// A problem in a skin file. Radlet skips that one value and carries on.
public struct SkinIssue: Equatable, Sendable, CustomStringConvertible {
    public let line: Int?
    public let message: String

    public init(line: Int?, _ message: String) {
        self.line = line
        self.message = message
    }

    public var description: String { line.map { "Line \($0): \(message)" } ?? message }
}

/// Everything a skin can set. The defaults are the Glass skin: the plain
/// system look, see-through, in the Mac's own colours and font.
public struct Skin: Equatable, Sendable, Identifiable {
    public enum Material: String, CaseIterable, Sendable { case solid, glass }
    public enum Shape: String, CaseIterable, Sendable { case round, faceted }
    public enum Highlight: String, CaseIterable, Sendable { case wedge, fill, rim }
    public enum Labels: String, CaseIterable, Sendable { case text, icons }

    /// "glass" for a built-in, "file:Name.css" for one in the Skins folder.
    public var id: String = "glass"
    public var name: String = "Glass"
    public var author: String = "Radlet"
    /// The built-in it starts from.
    public var base: String = "glass"

    public var accent = SkinColor.system(.accent)
    public var accentText = SkinColor(hex: 0xFFFFFF)
    public var text = SkinColor.system(.label)
    public var textDim = SkinColor.system(.secondaryLabel)
    public var ground = SkinColor.rgba(1, 1, 1, 0.04)
    public var ground2 = SkinColor(hex: 0x808080, alpha: 0.10)
    public var segment = SkinColor(hex: 0x808080, alpha: 0.05)
    public var divider = SkinColor.system(.separator)
    public var bezel = SkinColor.rgba(1, 1, 1, 0.55)
    public var bezel2 = SkinColor.rgba(1, 1, 1, 0.12)
    public var ticks = SkinColor.rgba(0, 0, 0, 0)
    public var glow = SkinColor.rgba(0, 0, 0, 0)
    public var shadow = SkinColor.rgba(0, 0, 0, 0.25)

    public var material = Material.glass
    public var shape = Shape.round
    public var highlight = Highlight.fill
    public var labels = Labels.icons
    public var uppercase = false
    public var scanlines = false

    public var tickCount = 0
    public var bezelWidth = 1.0
    public var segmentGap = 0.7
    public var glowRadius = 0.0
    public var highlightOpacity = 0.14
    public var labelSize = 11.0
    public var labelWeight = 600
    public var letterSpacing = 0.0
    public var springResponse = 0.25
    public var springDamping = 0.8
    /// Milliseconds between labels appearing.
    public var stagger = 14.0
    public var font = SkinFont.system(.standard)

    public init() {}

    public var isBuiltIn: Bool { !id.hasPrefix("file:") }

    /// The skin as a complete, commented file: what "New Skin" writes.
    public func css(base: String? = nil) -> String {
        var lines = [
            "/* Radlet skin. Change any value and save: Radlet picks it up the next time",
            "   the wheel opens. Anything you remove comes from --base.",
            "   Colours: #hex, rgb(), rgba(), hsl(), or the Mac's own colours:",
            "   system-accent, label, secondary-label, tertiary-label, separator,",
            "   window, control. Fonts: a family installed on this Mac in quotes,",
            "   or system, system-rounded, system-serif, system-mono. */",
            "",
            ":root {",
        ]
        var group = ""
        for token in SkinCSS.tokens {
            if token.group != group {
                if group != "" { lines.append("") }
                group = token.group
                lines.append("  /* \(group) */")
            }
            let value = token.name == "--base" ? (base ?? self.base) : token.write(self)
            let decl = "  \(token.name): \(value);"
            let pad = String(repeating: " ", count: max(1, 40 - decl.count))
            lines.append("\(decl)\(pad)/* \(token.help) */")
        }
        lines.append("}")
        return lines.joined(separator: "\n") + "\n"
    }

    /// The skin as a short file: its name and base, then only the tokens
    /// that differ from that base. What the Wheel Lab copies and saves.
    public func compactCSS() -> String {
        let from = SkinLibrary.base(of: self)
        func quoted(_ s: String, _ max: Int) -> String {
            "\"\(SkinCSS.cleanText(s, max: max).replacingOccurrences(of: "\"", with: "'"))\""
        }
        var lines = [":root {", "  --name: \(quoted(name, 40));"]
        if !author.isEmpty { lines.append("  --author: \(quoted(author, 60));") }
        lines.append("  --base: \(from.id);")
        for token in SkinCSS.editable where token.write(self) != token.write(from) {
            lines.append("  \(token.name): \(token.write(self));")
        }
        lines.append("}")
        return lines.joined(separator: "\n") + "\n"
    }
}

enum SkinNumber {
    static func format(_ v: Double) -> String {
        if v == v.rounded(), abs(v) < 1e9 { return String(Int(v)) }
        var s = String(format: "%.3f", v)
        while s.hasSuffix("0") { s.removeLast() }
        if s.hasSuffix(".") { s.removeLast() }
        return s
    }

    /// "12px" → (12, "px"). Nil for anything that isn't a number.
    static func split(_ raw: String) -> (Double, String)? {
        let s = raw.trimmingCharacters(in: .whitespaces).lowercased()
        var end = s.startIndex
        for i in s.indices {
            let c = s[i]
            if c.isNumber || c == "." || ((c == "-" || c == "+") && i == s.startIndex) {
                end = s.index(after: i)
            } else {
                break
            }
        }
        guard let v = Double(s[..<end]), v.isFinite else { return nil }
        return (v, String(s[end...]).trimmingCharacters(in: .whitespaces))
    }
}

/// Reads and writes skin files.
public enum SkinCSS {
    /// One custom property a skin can set.
    public struct Token {
        /// What sort of value a token takes, so an editor can offer the
        /// right control.
        public enum Kind: Equatable, Sendable {
            case text
            /// --base: the built-in skin a file starts from.
            case base
            case color
            case font
            /// One of these words.
            case choice([String])
            /// A number in `unit`; `step` is a sensible increment.
            case number(ClosedRange<Double>, unit: String, step: Double)
        }

        public let name: String
        /// Colours, Look, Type, Motion or About.
        public let group: String
        public let help: String
        public let kind: Kind
        /// Applies a value; returns what's wrong with it, if anything.
        let read: (inout Skin, String) -> String?
        let write: (Skin) -> String

        /// `skin`'s value, written as a skin file would.
        public func value(in skin: Skin) -> String { write(skin) }

        /// Applies a CSS value to `skin` as a skin file would: a number out
        /// of range is clamped, anything unreadable leaves `skin` alone.
        /// Returns what's wrong with the value, if anything. No var().
        public func apply(_ value: String, to skin: inout Skin) -> String? { read(&skin, value) }

        /// A number token's value in its unit, such as 0.3 for "0.3s".
        public func number(in skin: Skin) -> Double? {
            guard case .number = kind else { return nil }
            return SkinNumber.split(write(skin))?.0
        }

        /// A number written in this token's unit, such as "0.3s".
        public func text(for number: Double) -> String {
            guard case .number(_, let unit, _) = kind else { return SkinNumber.format(number) }
            return SkinNumber.format(number) + unit
        }
    }

    static func color(_ name: String, _ kp: WritableKeyPath<Skin, SkinColor>, _ help: String) -> Token {
        Token(name: name, group: "Colours", help: help, kind: .color, read: { skin, value in
            guard let c = SkinColor(css: value) else { return "isn't a colour Radlet knows" }
            skin[keyPath: kp] = c
            return nil
        }, write: { $0[keyPath: kp].css })
    }

    static func keyword<E: RawRepresentable & CaseIterable>(_ name: String, _ kp: WritableKeyPath<Skin, E>,
                                                             _ help: String) -> Token where E.RawValue == String {
        Token(name: name, group: "Look", help: help, kind: .choice(E.allCases.map(\.rawValue)), read: { skin, value in
            guard let e = E(rawValue: unquote(value).lowercased()) else {
                return "should be one of: \(E.allCases.map(\.rawValue).joined(separator: ", "))"
            }
            skin[keyPath: kp] = e
            return nil
        }, write: { $0[keyPath: kp].rawValue })
    }

    static func flag(_ name: String, _ kp: WritableKeyPath<Skin, Bool>, on: String, off: String, _ help: String) -> Token {
        Token(name: name, group: "Look", help: help, kind: .choice([off, on]), read: { skin, value in
            switch unquote(value).lowercased() {
            case on, "on", "true", "yes": skin[keyPath: kp] = true
            case off, "off", "false", "no", "none": skin[keyPath: kp] = false
            default: return "should be \(on) or \(off)"
            }
            return nil
        }, write: { $0[keyPath: kp] ? on : off })
    }

    /// A number in `unit` (`units` maps each accepted unit to it).
    static func number(_ name: String, _ group: String, _ kp: WritableKeyPath<Skin, Double>, _ range: ClosedRange<Double>,
                       unit: String, units: [String: Double], step: Double, _ help: String) -> Token {
        Token(name: name, group: group, help: help, kind: .number(range, unit: unit, step: step), read: { skin, value in
            guard let (v, u) = SkinNumber.split(value) else { return "isn't a number" }
            guard let scale = units[u] else {
                return "has an unknown unit “\(u)”; use \(units.keys.filter { !$0.isEmpty }.sorted().joined(separator: " or "))"
            }
            let n = v * scale
            let clamped = min(max(n, range.lowerBound), range.upperBound)
            skin[keyPath: kp] = clamped
            if clamped != n {
                return "is outside \(SkinNumber.format(range.lowerBound))–\(SkinNumber.format(range.upperBound))\(unit); using \(SkinNumber.format(clamped))\(unit)"
            }
            return nil
        }, write: { SkinNumber.format($0[keyPath: kp]) + unit })
    }

    static func integer(_ name: String, _ group: String, _ kp: WritableKeyPath<Skin, Int>, _ range: ClosedRange<Int>,
                        words: [String: Int] = [:], step: Int = 1, _ help: String) -> Token {
        Token(name: name, group: group, help: help,
              kind: .number(Double(range.lowerBound)...Double(range.upperBound), unit: "", step: Double(step)),
              read: { skin, value in
            let word = unquote(value).lowercased()
            let n: Double
            if let w = words[word] {
                n = Double(w)
            } else if let (v, u) = SkinNumber.split(word), u.isEmpty {
                n = v.rounded()
            } else {
                return "isn't a whole number"
            }
            let clamped = Int(min(max(n, Double(range.lowerBound)), Double(range.upperBound)))
            skin[keyPath: kp] = clamped
            return Double(clamped) == n ? nil : "is outside \(range.lowerBound)–\(range.upperBound); using \(clamped)"
        }, write: { String($0[keyPath: kp]) })
    }

    static func text(_ name: String, _ kp: WritableKeyPath<Skin, String>, max: Int, _ help: String) -> Token {
        Token(name: name, group: "About", help: help, kind: .text, read: { skin, value in
            skin[keyPath: kp] = cleanText(unquote(value), max: max)
            return nil
        }, write: { "\"\($0[keyPath: kp].replacingOccurrences(of: "\"", with: "'"))\"" })
    }

    static let px = ["": 1.0, "px": 1.0]
    static let seconds = ["": 1.0, "s": 1.0, "ms": 0.001]
    static let millis = ["": 1.0, "ms": 1.0, "s": 1000.0]

    /// Every token, in the order a written file lists them.
    static let tokens: [Token] = [
        text("--name", \.name, max: 40, "shown in Settings"),
        text("--author", \.author, max: 60, "you"),
        Token(name: "--base", group: "About", help: "the built-in skin this starts from", kind: .base,
              read: { _, _ in nil }, write: { $0.base }),

        color("--accent", \.accent, "the highlight"),
        color("--accent-text", \.accentText, "text on the accent (fill highlight)"),
        color("--text", \.text, "labels"),
        color("--text-dim", \.textDim, "secondary text"),
        color("--ground", \.ground, "the disc"),
        color("--ground-2", \.ground2, "the hub, and the disc's sheen"),
        color("--segment", \.segment, "each slot's fill"),
        color("--divider", \.divider, "lines between slots"),
        color("--bezel", \.bezel, "the rim, light side"),
        color("--bezel-2", \.bezel2, "the rim, dark side"),
        color("--ticks", \.ticks, "tick marks on the rim"),
        color("--glow", \.glow, "glow around the highlight"),
        color("--shadow", \.shadow, "shadow under the wheel"),

        keyword("--material", \Skin.material, "solid, or glass (see-through)"),
        keyword("--shape", \Skin.shape, "round, or faceted (one flat edge per slot)"),
        keyword("--highlight", \Skin.highlight, "wedge, fill or rim"),
        keyword("--labels", \Skin.labels, "text, or icons with text"),
        flag("--label-case", \.uppercase, on: "upper", off: "none", "upper or none"),
        flag("--scanlines", \.scanlines, on: "on", off: "off", "on or off"),
        integer("--tick-count", "Look", \.tickCount, 0...120, "marks round the rim, 0 for none"),
        number("--bezel-width", "Look", \.bezelWidth, 0...24, unit: "px", units: px, step: 1, "rim thickness, 0–24px"),
        number("--segment-gap", "Look", \.segmentGap, 0...6, unit: "deg", units: ["": 1, "deg": 1], step: 0.1,
               "gap between slots, 0–6deg"),
        number("--glow-radius", "Look", \.glowRadius, 0...40, unit: "px", units: px, step: 1, "0–40px"),
        number("--highlight-opacity", "Look", \.highlightOpacity, 0...1, unit: "", units: ["": 1, "%": 0.01], step: 0.01,
               "wedge fill strength, 0–1"),

        Token(name: "--font", group: "Type", help: "system, system-mono, or \"Family Name\"", kind: .font,
              read: { skin, value in
                  guard let f = font(value) else { return "isn't a font name" }
                  skin.font = f
                  return nil
              }, write: { $0.font.css }),
        number("--label-size", "Type", \.labelSize, 9...20, unit: "px", units: px, step: 0.5, "9–20px"),
        integer("--label-weight", "Type", \.labelWeight, 100...900,
                words: ["thin": 100, "light": 300, "normal": 400, "regular": 400, "medium": 500,
                        "semibold": 600, "bold": 700, "heavy": 800, "black": 900], step: 100, "100–900"),
        number("--letter-spacing", "Type", \.letterSpacing, -0.1...0.5, unit: "em", units: ["": 1, "em": 1], step: 0.01,
               "-0.1–0.5em"),

        number("--spring-response", "Motion", \.springResponse, 0.1...1.2, unit: "s", units: seconds, step: 0.01,
               "how long the highlight takes to glide"),
        number("--spring-damping", "Motion", \.springDamping, 0.3...1, unit: "", units: ["": 1], step: 0.01,
               "1 settles without bouncing, lower bounces"),
        number("--stagger", "Motion", \.stagger, 0...60, unit: "ms", units: millis, step: 1,
               "delay between labels appearing"),
    ]

    /// The tokens an editor offers: everything but name, author and base.
    public static var editable: [Token] { tokens.filter { $0.group != "About" } }

    /// Text for a name: one line, no control characters, at most `max` long.
    static func cleanText(_ s: String, max: Int) -> String {
        let kept = s.filter { !$0.isNewline && !($0.asciiValue.map { $0 < 32 || $0 == 127 } ?? false) }
        return String(kept.prefix(max)).trimmingCharacters(in: .whitespaces)
    }

    static func unquote(_ s: String) -> String {
        let t = s.trimmingCharacters(in: .whitespaces)
        if t.count >= 2, let f = t.first, let l = t.last, f == l, f == "\"" || f == "'" {
            return String(t.dropFirst().dropLast())
        }
        return t
    }

    static func font(_ raw: String) -> SkinFont? {
        let t = raw.trimmingCharacters(in: .whitespaces)
        let quoted = t.first == "\"" || t.first == "'"
        if !quoted, let d = SkinFont.Design(rawValue: t.lowercased()) { return .system(d) }
        let name = unquote(t)
        guard !name.isEmpty, name.count <= 64, !name.contains(where: { $0.isNewline || $0 == ";" }) else { return nil }
        return .named(name)
    }

    /// Reads a skin file. Problems are reported and skipped; the rest of
    /// the file still applies. `bases` are the skins `--base` can name.
    public static func parse(_ source: String, id: String, fallbackName: String,
                             bases: [Skin] = []) -> (skin: Skin, issues: [SkinIssue]) {
        var issues: [SkinIssue] = []
        let decls = declarations(stripComments(source, &issues), &issues)

        var raw: [String: String] = [:]
        for d in decls { raw[d.name] = d.value }

        var skin = Skin()
        if let b = decls.last(where: { $0.name == "--base" }) {
            let key = unquote(b.value).lowercased()
            if let found = bases.first(where: { $0.id == key }) {
                skin = found
            } else if key != SkinLibrary.defaultID {
                issues.append(SkinIssue(line: b.line, "--base “\(key)” isn't a built-in skin; using \(SkinLibrary.defaultID)"))
            }
            skin.base = bases.contains { $0.id == key } ? key : SkinLibrary.defaultID
        }
        // var() can name any token: set in this file, or else the base's.
        var resolver = Resolver(raw: raw, base: skin)
        // Name and author are the file's own, never the base's.
        skin.name = ""
        skin.author = ""

        for d in decls where d.name != "--base" {
            guard let token = tokenByName[d.name] else {
                issues.append(SkinIssue(line: d.line, "\(d.name.prefix(40)) isn't a Radlet token"))
                continue
            }
            guard let value = resolver.expand(d.value) else {
                issues.append(SkinIssue(line: d.line, "\(d.name) uses var() of a token that isn't set, or loops"))
                continue
            }
            if let problem = token.read(&skin, value) {
                issues.append(SkinIssue(line: d.line, "\(d.name) \(problem)"))
            }
        }
        if skin.name.isEmpty { skin.name = cleanText(fallbackName, max: 40) }
        skin.id = id
        return (skin, issues)
    }

    static let tokenByName: [String: Token] = Dictionary(tokens.map { ($0.name, $0) }, uniquingKeysWith: { a, _ in a })

    struct Declaration {
        let name: String
        let value: String
        let line: Int
    }

    /// Expands var(--name) and var(--name, fallback). Each name is worked
    /// out once; loops, unknown names and results over a kilobyte fail.
    struct Resolver {
        let raw: [String: String]
        let base: Skin
        private var memo: [String: String?] = [:]
        private var visiting: Set<String> = []
        static let maxLength = 1024

        init(raw: [String: String], base: Skin) {
            self.raw = raw
            self.base = base
        }

        mutating func value(of name: String) -> String? {
            if let known = memo[name] { return known }
            guard !visiting.contains(name) else { return nil }
            visiting.insert(name)
            let result: String?
            if let v = raw[name] {
                result = expand(v)
            } else if name != "--base", let token = SkinCSS.tokenByName[name] {
                result = token.write(base)
            } else {
                result = nil
            }
            visiting.remove(name)
            memo[name] = result
            return result
        }

        mutating func expand(_ text: String) -> String? {
            guard text.range(of: "var(", options: .caseInsensitive) != nil else { return text }
            var out = ""
            var rest = Substring(text)
            while let r = rest.range(of: "var(", options: .caseInsensitive) {
                out += rest[..<r.lowerBound]
                var level = 1
                var end = r.upperBound
                while end < rest.endIndex {
                    if rest[end] == "(" { level += 1 }
                    if rest[end] == ")" {
                        level -= 1
                        if level == 0 { break }
                    }
                    end = rest.index(after: end)
                }
                guard end < rest.endIndex else { return nil }
                let inside = rest[r.upperBound..<end]
                let name: String
                var fallback: Substring?
                if let comma = inside.firstIndex(of: ",") {
                    name = inside[..<comma].trimmingCharacters(in: .whitespaces).lowercased()
                    fallback = inside[inside.index(after: comma)...]
                } else {
                    name = inside.trimmingCharacters(in: .whitespaces).lowercased()
                }
                var replacement = value(of: name)
                if replacement == nil, let f = fallback {
                    replacement = expand(f.trimmingCharacters(in: .whitespaces))
                }
                guard let found = replacement else { return nil }
                out += found
                guard out.utf8.count <= Resolver.maxLength else { return nil }
                rest = rest[rest.index(after: end)...]
            }
            out += rest
            return out.utf8.count <= Resolver.maxLength ? out : nil
        }
    }

    /// Replaces comments with spaces, keeping line breaks so line numbers
    /// stay right. A /* inside quotes is text, not a comment.
    static func stripComments(_ s: String, _ issues: inout [SkinIssue]) -> String {
        var out = ""
        out.reserveCapacity(s.utf8.count)
        let chars = Array(s)
        var i = 0
        var line = 1
        var quote: Character?
        while i < chars.count {
            let c = chars[i]
            if c.isNewline {
                line += 1
                quote = nil    // CSS strings end at the line
            } else if let q = quote {
                if c == q { quote = nil }
            } else if c == "\"" || c == "'" {
                quote = c
            } else if c == "/", i + 1 < chars.count, chars[i + 1] == "*" {
                let start = line
                i += 2
                var closed = false
                while i < chars.count {
                    if chars[i] == "*", i + 1 < chars.count, chars[i + 1] == "/" {
                        i += 2
                        closed = true
                        break
                    }
                    if chars[i].isNewline {
                        out.append(chars[i])
                        line += 1
                    }
                    i += 1
                }
                if !closed { issues.append(SkinIssue(line: start, "a comment is never closed with */")) }
                out.append(" ")
                continue
            }
            out.append(c)
            i += 1
        }
        return out
    }

    /// The declarations inside `:root { … }` blocks, in order. Braces
    /// inside quotes are text.
    static func declarations(_ s: String, _ issues: inout [SkinIssue]) -> [Declaration] {
        var out: [Declaration] = []
        let chars = Array(s)
        var i = 0
        var line = 1
        var selector = ""
        var selectorLine = 1
        var started = false
        var quote: Character?
        while i < chars.count {
            let c = chars[i]
            if c.isNewline {
                line += 1
                quote = nil
            } else if let q = quote {
                if c == q { quote = nil }
            } else if c == "\"" || c == "'" {
                quote = c
            } else if c == "{" {
                // Find the matching brace; Radlet's files never nest.
                var j = i + 1
                var bodyLine = line
                var body: [(Character, Int)] = []
                var depth = 1
                var nested = false
                var inner: Character?
                while j < chars.count {
                    let d = chars[j]
                    if d.isNewline {
                        inner = nil
                    } else if let q = inner {
                        if d == q { inner = nil }
                    } else if d == "\"" || d == "'" {
                        inner = d
                    } else if d == "{" {
                        depth += 1
                        nested = true
                    } else if d == "}" {
                        depth -= 1
                        if depth == 0 { break }
                    }
                    body.append((d, bodyLine))
                    if d.isNewline { bodyLine += 1 }
                    j += 1
                }
                let name = selector.trimmingCharacters(in: .whitespacesAndNewlines)
                let selectors = name.split(separator: ",").map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                if j >= chars.count {
                    issues.append(SkinIssue(line: selectorLine, "“\(name.prefix(30))” is missing its closing }"))
                }
                if nested {
                    issues.append(SkinIssue(line: selectorLine, "blocks can't be nested; skipped “\(name.prefix(30))”"))
                } else if selectors.contains(":root") || selectors.contains(".wheel") {
                    out += split(body, &issues)
                } else {
                    issues.append(SkinIssue(line: selectorLine, "Radlet reads only :root { … }; skipped “\(name.prefix(30))”"))
                }
                line = bodyLine
                i = j + 1
                selector = ""
                started = false
                continue
            } else if c == "}" {
                issues.append(SkinIssue(line: line, "a } without a matching {"))
                i += 1
                continue
            }
            if !started, !c.isWhitespace {
                started = true
                selectorLine = line
            }
            if started, selector.utf8.count < 200 { selector.append(c) }
            i += 1
        }
        let leftover = selector.trimmingCharacters(in: .whitespacesAndNewlines)
        if !leftover.isEmpty {
            issues.append(SkinIssue(line: selectorLine, "“\(leftover.prefix(30))” isn't inside :root { … }"))
        }
        return out
    }

    /// Splits a block's body at semicolons outside quotes and brackets.
    static func split(_ body: [(Character, Int)], _ issues: inout [SkinIssue]) -> [Declaration] {
        var out: [Declaration] = []
        var current = ""
        var startLine: Int?
        var quote: Character?
        var parens = 0
        func finish() {
            let text = current.trimmingCharacters(in: .whitespacesAndNewlines)
            defer {
                current = ""
                startLine = nil
            }
            guard !text.isEmpty else { return }
            let line = startLine ?? 0
            guard let colon = text.firstIndex(of: ":") else {
                issues.append(SkinIssue(line: line, "“\(text.prefix(30))” has no value (expected name: value)"))
                return
            }
            let name = text[..<colon].trimmingCharacters(in: .whitespaces).lowercased()
            var value = text[text.index(after: colon)...].trimmingCharacters(in: .whitespacesAndNewlines)
            if value.lowercased().hasSuffix("!important") {
                value = String(value.dropLast("!important".count)).trimmingCharacters(in: .whitespaces)
            }
            guard name.hasPrefix("--") else {
                issues.append(SkinIssue(line: line, "“\(name.prefix(30))” isn't a Radlet token; tokens start with --"))
                return
            }
            guard !value.isEmpty else {
                issues.append(SkinIssue(line: line, "\(name.prefix(40)) has no value"))
                return
            }
            out.append(Declaration(name: name, value: value, line: line))
        }
        for (c, line) in body {
            if startLine == nil, !c.isWhitespace { startLine = line }
            if c.isNewline {
                quote = nil
            } else if let q = quote {
                if c == q { quote = nil }
            } else if c == "\"" || c == "'" {
                quote = c
            } else if c == "(" {
                parens += 1
            } else if c == ")" {
                parens = max(0, parens - 1)
            } else if c == ";", parens == 0 {
                finish()
                continue
            }
            current.append(c)
        }
        finish()
        return out
    }
}

/// A skin with the file it came from and anything wrong in it.
public struct SkinFile: Equatable, Sendable, Identifiable {
    public let skin: Skin
    public let issues: [SkinIssue]
    public let url: URL?

    public var id: String { skin.id }
}

/// The built-in skins and the ones in the Skins folder.
public enum SkinLibrary {
    /// Skin files bigger than this are skipped.
    public static let maxBytes = 64 * 1024
    public static let maxFiles = 200
    /// Glass: the plain system look, as the wheel first shipped.
    public static let defaultID = "glass"

    /// Original looks. The game-flavoured ones nod to a genre without
    /// borrowing any game's art, names or fonts.
    static let builtInSources: [(id: String, css: String)] = [
        ("glass", """
        :root {
          --name: "Glass";
          --author: "Radlet";
          /* The plain system look: see-through, in the Mac's own colours,
             accent and font, following light and dark mode. */
        }
        """),
        ("instrument", """
        :root {
          --name: "Instrument";
          --author: "Radlet";
          /* Graphite, a machined rim and one bright accent. */
          --material: solid;
          --accent: #C5F05C;
          --accent-text: #0B0C0E;
          --text: #ECEEF0;
          --text-dim: #8D9299;
          --ground: #131518;
          --ground-2: #1C1F23;
          --segment: rgba(255, 255, 255, 0.02);
          --divider: rgba(255, 255, 255, 0.09);
          --bezel: #4B5057;
          --bezel-2: #202327;
          --ticks: rgba(255, 255, 255, 0.22);
          --glow: rgba(197, 240, 92, 0.55);
          --shadow: rgba(0, 0, 0, 0.55);
          --highlight: wedge;
          --labels: text;
          --tick-count: 60;
          --bezel-width: 10px;
          --segment-gap: 0;
          --glow-radius: 10px;
          --font: system-mono;
          --label-size: 14px;
          --label-weight: medium;
          --letter-spacing: 0.02em;
          --spring-response: 0.3s;
          --spring-damping: 0.72;
        }
        """),
        ("tactical", """
        :root {
          --name: "Tactical";
          --author: "Radlet";
          --base: instrument;
          /* Field kit: olive drab, amber readouts, flat edges, no bounce. */
          --accent: #F2B33D;
          --accent-text: #15140F;
          --text: #E6E2D3;
          --text-dim: #A39E88;
          --ground: #1A1C17;
          --ground-2: #24271F;
          --segment: rgba(230, 226, 211, 0.03);
          --divider: rgba(230, 226, 211, 0.14);
          --bezel: #43473B;
          --bezel-2: #1C1E18;
          --ticks: rgba(242, 179, 61, 0.38);
          --glow: rgba(242, 179, 61, 0.25);
          --shape: faceted;
          --highlight-opacity: 0.12;
          --label-case: upper;
          --tick-count: 72;
          --bezel-width: 6px;
          --segment-gap: 1.2deg;
          --glow-radius: 4px;
          --font: "Avenir Next Condensed";
          --label-size: 13px;
          --label-weight: bold;
          --letter-spacing: 0.12em;
          --spring-response: 0.22s;
          --spring-damping: 0.92;
          --stagger: 8ms;
        }
        """),
        ("neon", """
        :root {
          --name: "Neon";
          --author: "Radlet";
          --base: instrument;
          /* Night-market signage: magenta and cyan on ink, with scanlines. */
          --accent: #FF2BD6;
          --accent-text: #14001A;
          --text: #F3E9FF;
          --text-dim: #A797C7;
          --ground: #0B0716;
          --ground-2: #160E2C;
          --segment: rgba(51, 240, 255, 0.03);
          --divider: rgba(51, 240, 255, 0.18);
          --bezel: #33F0FF;
          --bezel-2: var(--accent);
          --ticks: rgba(51, 240, 255, 0.5);
          --glow: rgba(255, 43, 214, 0.85);
          --shape: faceted;
          --highlight-opacity: 0.22;
          --label-case: upper;
          --scanlines: on;
          --tick-count: 90;
          --bezel-width: 3px;
          --segment-gap: 0.8deg;
          --glow-radius: 18px;
          --font: system-mono;
          --label-weight: semibold;
          --letter-spacing: 0.08em;
          --spring-response: 0.26s;
          --spring-damping: 0.62;
          --stagger: 18ms;
        }
        """),
        ("sunset", """
        :root {
          --name: "Sunset";
          --author: "Radlet";
          --base: instrument;
          /* Coastal dusk: hot pink and teal over plum, chunky type. */
          --accent: #FF5FA2;
          --accent-text: #2A0B1C;
          --text: #FFF1F6;
          --text-dim: #D9AEC4;
          --ground: rgba(38, 16, 40, 0.94);
          --ground-2: #3A1840;
          --segment: rgba(255, 255, 255, 0.03);
          --divider: rgba(255, 182, 213, 0.16);
          --bezel: #FF9A5C;
          --bezel-2: #FF4FA0;
          --ticks: rgba(46, 216, 195, 0.55);
          --glow: rgba(255, 95, 162, 0.6);
          --highlight: fill;
          --labels: icons;
          --label-case: upper;
          --tick-count: 48;
          --bezel-width: 8px;
          --segment-gap: 1.5deg;
          --glow-radius: 14px;
          --font: "Futura";
          --label-size: 11px;
          --label-weight: bold;
          --letter-spacing: 0.06em;
          --spring-damping: 0.68;
        }
        """),
        ("porcelain", """
        :root {
          --name: "Porcelain";
          --author: "Radlet";
          --base: instrument;
          /* Light: warm white, a steel rim and a blue accent. */
          --accent: #2F6BFF;
          --accent-text: #FFFFFF;
          --text: #1B1C1E;
          --text-dim: #63676E;
          --ground: #F5F3EF;
          --ground-2: #FFFFFF;
          --segment: rgba(0, 0, 0, 0.02);
          --divider: rgba(0, 0, 0, 0.09);
          --bezel: #E4E1DB;
          --bezel-2: #B4B0A8;
          --ticks: rgba(0, 0, 0, 0.25);
          --glow: rgba(47, 107, 255, 0.25);
          --shadow: rgba(60, 50, 40, 0.25);
          --highlight-opacity: 0.12;
          --bezel-width: 8px;
          --glow-radius: 6px;
          --font: system;
          --label-weight: semibold;
          --letter-spacing: 0;
        }
        """),
    ]

    /// Each can start from one listed before it.
    public static let builtIns: [SkinFile] = builtInSources.reduce(into: []) { done, entry in
        let parsed = SkinCSS.parse(entry.css, id: entry.id, fallbackName: entry.id.capitalized, bases: done.map(\.skin))
        var skin = parsed.skin
        skin.base = entry.id
        done.append(SkinFile(skin: skin, issues: parsed.issues, url: nil))
    }

    public static var builtInSkins: [Skin] { builtIns.map(\.skin) }

    /// The built-in skin `skin` starts from (Glass if it names none).
    public static func base(of skin: Skin) -> Skin {
        builtInSkins.first { $0.id == skin.base } ?? builtIns[0].skin
    }

    /// Where skins you make or download live.
    public static var defaultDirectory: URL {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent("Library/Application Support")
        return support.appendingPathComponent("Radlet/Skins", isDirectory: true)
    }

    /// Reads every .css file in `directory`, sorted by name. Only regular
    /// files are read (a link is followed to one), and at most `maxBytes`.
    public static func load(directory: URL) -> [SkinFile] {
        let fm = FileManager.default
        guard let names = try? fm.contentsOfDirectory(atPath: directory.path) else { return [] }
        let files = names.filter { $0.lowercased().hasSuffix(".css") && !$0.hasPrefix(".") }
            .sorted { $0.localizedStandardCompare($1) == .orderedAscending }
            .prefix(maxFiles)
        return files.map { name in
            let url = directory.appendingPathComponent(name)
            let stem = String(name.dropLast(4))
            let id = "file:\(name)"
            func skipped(_ why: String) -> SkinFile {
                var skin = Skin()
                skin.id = id
                skin.name = SkinCSS.cleanText(stem, max: 40)
                return SkinFile(skin: skin, issues: [SkinIssue(line: nil, why)], url: url)
            }
            switch read(url) {
            case .failure(let why):
                return skipped(why.message)
            case .success(let text):
                let parsed = SkinCSS.parse(text, id: id, fallbackName: stem, bases: builtInSkins)
                return SkinFile(skin: parsed.skin, issues: parsed.issues, url: url)
            }
        }
    }

    struct ReadProblem: Error {
        let message: String
    }

    /// A skin file's text, refusing anything that isn't a small regular
    /// UTF-8 file: a pipe or device could otherwise block or never end.
    static func read(_ url: URL) -> Result<String, ReadProblem> {
        let real = url.resolvingSymlinksInPath()
        let values = try? real.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey])
        guard values?.isRegularFile == true else { return .failure(ReadProblem(message: "this isn't a regular file, so Radlet skipped it")) }
        guard (values?.fileSize ?? 0) <= maxBytes else {
            return .failure(ReadProblem(message: "the file is over 64 KB, so Radlet skipped it"))
        }
        guard let handle = try? FileHandle(forReadingFrom: real) else {
            return .failure(ReadProblem(message: "Radlet couldn't open this file"))
        }
        defer { try? handle.close() }
        let data = (try? handle.read(upToCount: maxBytes + 1)) ?? Data()
        guard data.count <= maxBytes else { return .failure(ReadProblem(message: "the file is over 64 KB, so Radlet skipped it")) }
        guard let text = String(data: data, encoding: .utf8) else {
            return .failure(ReadProblem(message: "Radlet couldn't read this file as UTF-8 text"))
        }
        return .success(text)
    }

    /// When each skin file (or the file a link points to) last changed, to
    /// tell when to reload.
    public static func signature(directory: URL) -> [String: Date] {
        let fm = FileManager.default
        guard let names = try? fm.contentsOfDirectory(atPath: directory.path) else { return [:] }
        var out: [String: Date] = [:]
        for name in names where name.lowercased().hasSuffix(".css") {
            let real = directory.appendingPathComponent(name).resolvingSymlinksInPath()
            let date = (try? real.resourceValues(forKeys: [.contentModificationDateKey]))?.contentModificationDate
            out[name] = date ?? .distantPast
        }
        return out
    }

    /// A file name for a new skin that isn't taken: "My Skin.css", then
    /// "My Skin 2.css".
    public static func newFileURL(in directory: URL, name: String) -> URL {
        let cleaned = name.map { "/:\\".contains($0) || $0.isNewline ? "-" : $0 }
        var stem = String(cleaned).trimmingCharacters(in: .whitespaces)
        if stem.isEmpty || stem.hasPrefix(".") { stem = "My Skin" }
        stem = String(stem.prefix(60))
        let fm = FileManager.default
        var n = 1
        while true {
            let candidate = directory.appendingPathComponent(n == 1 ? "\(stem).css" : "\(stem) \(n).css")
            if !fm.fileExists(atPath: candidate.path) { return candidate }
            n += 1
        }
    }
}
