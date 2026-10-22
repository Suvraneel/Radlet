import AppKit
import SwiftUI
import VeerKit
import VeerUI

/// The editor for one skin token. The CSS value in `text` is what counts;
/// the slider, colour well or picker each write it, and are kept in step
/// with the skin after every change.
@MainActor
final class TokenField: ObservableObject, Identifiable {
    let token: SkinCSS.Token
    let id: String
    /// The token's name without the dashes.
    let label: String

    /// The CSS value, as typed. Applied on every change.
    @Published var text = "" {
        didSet { if !syncing, text != oldValue { submit() } }
    }
    @Published var number = 0.0 {
        didSet { if !syncing, number != oldValue { text = token.text(for: stepped(number)) } }
    }
    @Published var color = CGColor(gray: 0, alpha: 1) {
        didSet { if !syncing, color != oldValue { text = TokenField.css(color) } }
    }
    @Published var choice = "" {
        didSet { if !syncing, choice != oldValue { text = choice } }
    }
    /// What's wrong with the typed value, if anything.
    @Published private(set) var problem: String?
    /// Differs from the saved skin.
    @Published var changed = false

    /// Applies a value to the skin being edited; returns what's wrong with
    /// it and the skin as it now stands.
    var apply: @MainActor (SkinCSS.Token, String) -> (problem: String?, skin: Skin) = { _, _ in (nil, Skin()) }
    private var syncing = false

    init(_ token: SkinCSS.Token) {
        self.token = token
        id = token.name
        label = String(token.name.dropFirst(2))
    }

    /// Shows `skin`'s value without applying it.
    func show(_ skin: Skin) {
        syncing = true
        text = token.value(in: skin)
        problem = nil
        follow(skin)
        syncing = false
    }

    private func submit() {
        let result = apply(token, text)
        problem = result.problem
        syncing = true
        follow(result.skin)
        syncing = false
    }

    /// Moves the slider, colour well or picker to `skin`'s value.
    private func follow(_ skin: Skin) {
        switch token.kind {
        case .number:
            if let n = token.number(in: skin) { number = n }
        case .color:
            if let c = SkinColor(css: token.value(in: skin)) { color = c.nsColor.cgColor }
        case .choice:
            choice = token.value(in: skin)
        case .text, .base, .font:
            break
        }
    }

    /// A slider value rounded to the token's step, inside its range.
    private func stepped(_ v: Double) -> Double {
        guard case .number(let range, _, let step) = token.kind, step > 0 else { return v }
        let n = range.lowerBound + ((v - range.lowerBound) / step).rounded() * step
        return min(max(n, range.lowerBound), range.upperBound)
    }

    /// A colour from the colour well, as CSS.
    static func css(_ c: CGColor) -> String {
        guard let srgb = CGColorSpace(name: CGColorSpace.sRGB),
              let converted = c.converted(to: srgb, intent: .defaultIntent, options: nil),
              let parts = converted.components, parts.count >= 4 else {
            return SkinColor.rgba(0, 0, 0, 1).css
        }
        func unit(_ v: CGFloat) -> Double { min(max(Double(v), 0), 1) }
        return SkinColor.rgba(unit(parts[0]), unit(parts[1]), unit(parts[2]), unit(parts[3])).css
    }
}
