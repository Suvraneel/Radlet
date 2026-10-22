import AppKit
import SwiftUI
import VeerKit

// Drawing for the wheel's three styles. Every colour, size and motion comes
// from the skin (see VeerKit/Skin.swift); the geometry from WheelSpec.
// Shared by the app and the Wheel Lab, so the lab shows exactly what ships.

/// A skin ready to draw: colours converted once, the font looked up once.
package struct WheelLook {
    package let skin: Skin
    package let accent: Color
    package let accentText: Color
    package let text: Color
    package let textDim: Color
    package let ground: Color
    package let ground2: Color
    package let segment: Color
    package let divider: Color
    package let bezel: Color
    package let bezel2: Color
    package let ticks: Color
    package let glow: Color
    package let shadow: Color
    /// Nil uses the system font in `design`.
    private let fontName: String?
    private let design: Font.Design
    /// System setting: no springs or scaling, just quick fades.
    package let reduceMotion: Bool

    /// Main actor: it asks AppKit whether the skin's font is installed.
    /// `reduceMotion` overrides the system setting (the Wheel Lab).
    @MainActor
    package init(_ skin: Skin, reduceMotion: Bool? = nil) {
        self.skin = skin
        accent = skin.accent.color
        accentText = skin.accentText.color
        text = skin.text.color
        textDim = skin.textDim.color
        ground = skin.ground.color
        ground2 = skin.ground2.color
        segment = skin.segment.color
        divider = skin.divider.color
        bezel = skin.bezel.color
        bezel2 = skin.bezel2.color
        ticks = skin.ticks.color
        glow = skin.glow.color
        shadow = skin.shadow.color
        self.reduceMotion = reduceMotion ?? NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        switch skin.font {
        case .system(let d):
            fontName = nil
            design = d.swiftUI
        case .named(let name):
            let installed = NSFontManager.shared.availableFontFamilies.contains(name) || NSFont(name: name, size: 12) != nil
            fontName = installed ? name : nil
            design = .default
        }
    }

    package func font(_ size: Double, weight: Int? = nil) -> Font {
        let w = WheelLook.weight(weight ?? skin.labelWeight)
        if let fontName { return .custom(fontName, size: size).weight(w) }
        return .system(size: size, weight: w, design: design)
    }

    package static func weight(_ v: Int) -> Font.Weight {
        switch v {
        case ..<150: return .ultraLight
        case ..<250: return .thin
        case ..<350: return .light
        case ..<450: return .regular
        case ..<550: return .medium
        case ..<650: return .semibold
        case ..<750: return .bold
        case ..<850: return .heavy
        default: return .black
        }
    }

    package var spring: Animation {
        reduceMotion ? .timingCurve(0.2, 0.8, 0.2, 1, duration: 0.12)
            : .spring(response: skin.springResponse, dampingFraction: skin.springDamping)
    }

    /// Sides for faceted shapes; 0 draws circles. Below six slots a
    /// polygon would cut through the labels, so those stay round.
    package func sides(_ spec: WheelSpec) -> Int { skin.shape == .faceted && spec.count >= 6 ? spec.count : 0 }

    /// Degrees between facet corners for SectorShape; 0 when round.
    package func facetSpan(_ spec: WheelSpec) -> Double { sides(spec) > 0 ? spec.span : 0 }

    /// Text on a highlighted slot.
    package var hotText: Color { skin.highlight == .fill ? accentText : accent }
}

extension SkinColor {
    package var color: Color {
        switch self {
        case let .rgba(r, g, b, a):
            return Color(.sRGB, red: r, green: g, blue: b, opacity: a)
        case .system:
            return Color(nsColor: nsColor)
        }
    }

    /// System colours stay dynamic: they follow light and dark mode.
    package var nsColor: NSColor {
        switch self {
        case let .rgba(r, g, b, a):
            return NSColor(srgbRed: r, green: g, blue: b, alpha: a)
        case .system(let s):
            switch s {
            case .accent: return .controlAccentColor
            case .label: return .labelColor
            case .secondaryLabel: return .secondaryLabelColor
            case .tertiaryLabel: return .tertiaryLabelColor
            case .separator: return .separatorColor
            case .window: return .windowBackgroundColor
            case .control: return .controlBackgroundColor
            }
        }
    }
}

extension SkinFont.Design {
    package var swiftUI: Font.Design {
        switch self {
        case .standard: return .default
        case .rounded: return .rounded
        case .serif: return .serif
        case .mono: return .monospaced
        }
    }
}

private func polar(_ c: CGPoint, _ r: Double, _ degrees: Double) -> CGPoint {
    let a = degrees * .pi / 180
    return CGPoint(x: Double(c.x) + r * sin(a), y: Double(c.y) - r * cos(a))
}

/// On a faceted wheel whose corners sit at radius `r` on the slot borders
/// (`span` apart, a slot's middle at `centre`), the distance from the
/// centre to the edge at angle `a`. Round wheels (span 0) just give `r`.
func facet(_ r: Double, _ a: Double, span: Double, centre: Double = 0) -> Double {
    guard span > 0 else { return r }
    let rel = WheelAngle.diff(a - centre, ((a - centre) / span).rounded() * span)
    return r * cos(span / 2 * .pi / 180) / cos(min(abs(rel), span / 2) * .pi / 180)
}

/// An annular sector from `start` to `end` degrees between two radii,
/// centred in its frame. Faceted sectors have straight edges, matching a
/// faceted ring whose corners sit on the slot borders.
struct SectorShape: Shape {
    var inner: Double
    var outer: Double
    var start: Double
    var end: Double
    /// Faceted: degrees between corners, and where a slot's middle falls
    /// in this shape's frame. 0 draws arcs.
    var facetSpan = 0.0
    var facetCentre = 0.0

    func path(in rect: CGRect) -> Path {
        let c = CGPoint(x: rect.midX, y: rect.midY)
        let faceted = facetSpan > 0
        let steps = faceted ? 1 : max(2, Int(abs(end - start) / 3))
        func at(_ r: Double, _ s: Int) -> CGPoint {
            let a = start + (end - start) * Double(s) / Double(steps)
            return polar(c, faceted ? facet(r, a, span: facetSpan, centre: facetCentre) : r, a)
        }
        var p = Path()
        for s in 0...steps {
            if s == 0 { p.move(to: at(outer, s)) } else { p.addLine(to: at(outer, s)) }
        }
        if inner > 0 {
            for s in stride(from: steps, through: 0, by: -1) { p.addLine(to: at(inner, s)) }
        } else {
            p.addLine(to: c)
        }
        p.closeSubpath()
        return p
    }
}

/// A disc or ring, round or with `sides` flat edges whose corners sit on
/// the slot borders. Fill rings with `FillStyle(eoFill: true)`.
struct RingShape: Shape {
    var inner: Double
    var outer: Double
    var sides = 0

    func path(in rect: CGRect) -> Path {
        let c = CGPoint(x: rect.midX, y: rect.midY)
        var p = Path()
        for r in [outer, inner] where r > 0 {
            if sides < 3 {
                p.addEllipse(in: CGRect(x: Double(c.x) - r, y: Double(c.y) - r, width: 2 * r, height: 2 * r))
            } else {
                let span = 360 / Double(sides)
                for k in 0...sides {
                    let pt = polar(c, r, Double(k) * span - span / 2)
                    if k == 0 { p.move(to: pt) } else { p.addLine(to: pt) }
                }
                p.closeSubpath()
            }
        }
        return p
    }
}

/// Straight lines at the given angles between two radii.
struct SpokesShape: Shape {
    var angles: [Double]
    var inner: Double
    var outer: Double

    func path(in rect: CGRect) -> Path {
        let c = CGPoint(x: rect.midX, y: rect.midY)
        var p = Path()
        for a in angles {
            p.move(to: polar(c, inner, a))
            p.addLine(to: polar(c, outer, a))
        }
        return p
    }
}

/// The disc: shadow, ground, bezel, ticks, slot fills and dividers, plus
/// Orbit's outer ring. Everything that doesn't move with the pointer.
struct WheelBase: View {
    let spec: WheelSpec
    let look: WheelLook
    /// Presets shown per position (Orbit's outer ring).
    let presets: [Int]

    var body: some View {
        let skin = look.skin
        let sides = look.sides(spec)
        let inside = spec.rim - skin.bezelWidth
        let gap = skin.segmentGap / 2
        let borders = (0..<spec.count).map { spec.angle(of: $0) - spec.span / 2 }
        ZStack {
            RingShape(inner: 0, outer: spec.rim, sides: sides)
                .fill(look.ground)
                .shadow(color: look.shadow, radius: 14, y: 6)
            if skin.material == .glass {
                RingShape(inner: 0, outer: spec.rim, sides: sides).fill(.regularMaterial)
                RingShape(inner: 0, outer: spec.rim, sides: sides).fill(look.ground)
            }
            RingShape(inner: 0, outer: inside, sides: sides)
                .fill(RadialGradient(colors: [look.ground2, look.ground], center: UnitPoint(x: 0.5, y: 0.3),
                                     startRadius: 0, endRadius: inside * 1.1))
                .opacity(skin.material == .glass ? 0.6 : 1)
            if skin.scanlines {
                Scanlines(color: look.text.opacity(0.05))
                    .clipShape(RingShape(inner: 0, outer: inside, sides: sides))
            }
            if skin.bezelWidth > 0 {
                RingShape(inner: inside, outer: spec.rim, sides: sides)
                    .fill(AngularGradient(colors: [look.bezel, look.bezel2, look.bezel, look.bezel2, look.bezel],
                                          center: .center, angle: .degrees(-70)),
                          style: FillStyle(eoFill: true))
            }
            Ticks(spec: spec, look: look, inner: spec.contentOuter + 2, outer: inside - 1, span: look.facetSpan(spec))
            ForEach(0..<spec.count, id: \.self) { i in
                SectorShape(inner: spec.hub, outer: spec.ringOuter,
                            start: spec.angle(of: i) - spec.span / 2 + gap, end: spec.angle(of: i) + spec.span / 2 - gap,
                            facetSpan: look.facetSpan(spec))
                    .fill(look.segment)
            }
            if spec.count > 1 && skin.segmentGap < 0.5 {
                SpokesShape(angles: borders, inner: spec.hub, outer: spec.ringOuter).stroke(look.divider, lineWidth: 1)
            }
            if spec.style == .orbit {
                OrbitBand(spec: spec, look: look, presets: presets)
            }
        }
        .frame(width: spec.size, height: spec.size)
    }
}

/// Orbit's outer ring: a groove, then one or two cells behind each slot.
private struct OrbitBand: View {
    let spec: WheelSpec
    let look: WheelLook
    let presets: [Int]

    var body: some View {
        let sides = look.sides(spec)
        let gap = look.skin.segmentGap / 2
        let cells: [(start: Double, end: Double)] = (0..<spec.count).flatMap { i -> [(start: Double, end: Double)] in
            let n = i < presets.count ? presets[i] : 0
            let a = spec.angle(of: i), h = spec.span / 2
            switch n {
            case 0: return []
            case 1: return [(a - h + gap, a + h - gap)]
            default: return [(a - h + gap, a - gap / 2), (a + gap / 2, a + h - gap)]
            }
        }
        ZStack {
            RingShape(inner: spec.ringOuter, outer: spec.presetInner, sides: sides)
                .fill(Color.black.opacity(0.28), style: FillStyle(eoFill: true))
            ForEach(Array(cells.enumerated()), id: \.offset) { _, cell in
                SectorShape(inner: spec.presetInner, outer: spec.presetOuter, start: cell.start, end: cell.end,
                            facetSpan: look.facetSpan(spec))
                    .fill(look.segment)
            }
            if spec.count > 1 && look.skin.segmentGap < 0.5 {
                SpokesShape(angles: (0..<spec.count).map { spec.angle(of: $0) - spec.span / 2 },
                            inner: spec.presetInner, outer: spec.presetOuter)
                    .stroke(look.divider, lineWidth: 1)
                SpokesShape(angles: (0..<spec.count).filter { $0 < presets.count && presets[$0] > 1 }.map { spec.angle(of: $0) },
                            inner: spec.presetInner, outer: spec.presetOuter)
                    .stroke(look.divider.opacity(0.5), lineWidth: 1)
            }
        }
    }
}

/// Tick marks round the rim: fine ones evenly spaced, strong ones on the
/// slot borders.
private struct Ticks: View {
    let spec: WheelSpec
    let look: WheelLook
    let inner: Double
    let outer: Double
    /// Faceted: degrees between corners, so ticks follow the flat edges.
    let span: Double

    var body: some View {
        Canvas { ctx, size in
            guard outer - inner >= 2 else { return }
            let c = CGPoint(x: size.width / 2, y: size.height / 2)
            func tick(_ path: inout Path, _ a: Double, from: Double) {
                path.move(to: polar(c, facet(from, a, span: span), a))
                path.addLine(to: polar(c, facet(outer, a, span: span), a))
            }
            let n = look.skin.tickCount
            if n > 0 {
                var minor = Path()
                for k in 0..<n { tick(&minor, Double(k) * 360 / Double(n), from: inner + (outer - inner) * 0.45) }
                ctx.stroke(minor, with: .color(look.ticks), lineWidth: 1)
            }
            if spec.count > 1, n > 0 {
                var major = Path()
                for i in 0..<spec.count { tick(&major, spec.angle(of: i) - spec.span / 2, from: inner) }
                ctx.stroke(major, with: .color(look.ticks), lineWidth: 2)
            }
        }
        .allowsHitTesting(false)
    }
}

private struct Scanlines: View {
    let color: Color

    var body: some View {
        Canvas { ctx, size in
            var p = Path()
            var y = 0.0
            while y < Double(size.height) {
                p.addRect(CGRect(x: 0, y: y, width: Double(size.width), height: 1))
                y += 3
            }
            ctx.fill(p, with: .color(color))
        }
        .allowsHitTesting(false)
    }
}

/// The highlight on the ring, drawn at 12 o'clock and turned to the slot,
/// so moving between slots glides instead of jumping.
struct RingHighlight: View {
    let spec: WheelSpec
    let look: WheelLook
    let inner: Double
    let outer: Double
    /// Degrees wide.
    let width: Double
    /// Dimmed: Orbit's parent slot while a preset is picked.
    let strong: Bool
    let flash: Bool
    /// Where the slot's middle sits relative to this highlight's middle
    /// (Orbit's half-slot presets are off-centre), for faceted edges.
    var centre = 0.0

    var body: some View {
        let skin = look.skin
        let fs = look.facetSpan(spec)
        let g = min(skin.segmentGap / 2, width / 4)
        let a0 = -width / 2 + g, a1 = width / 2 - g
        let rimWidth = skin.highlight == .rim ? 4.0 : 3.0
        ZStack {
            if skin.highlight != .rim {
                SectorShape(inner: inner + 1, outer: outer, start: a0, end: a1, facetSpan: fs, facetCentre: centre)
                    .fill(skin.highlight == .fill ? look.accent : look.accent.opacity(skin.highlightOpacity))
                    .shadow(color: skin.highlight == .fill ? look.glow : .clear, radius: skin.glowRadius)
                    .opacity(strong ? 1 : 0.45)
            }
            if skin.highlight != .fill && strong {
                SectorShape(inner: outer - rimWidth, outer: outer, start: a0, end: a1, facetSpan: fs, facetCentre: centre)
                    .fill(look.accent)
                    .shadow(color: look.glow, radius: skin.glowRadius)
            }
            SectorShape(inner: outer - rimWidth, outer: outer, start: a0, end: a1, facetSpan: fs, facetCentre: centre)
                .fill(Color.white)
                .opacity(flash && strong ? 0.85 : 0)
                .animation(flash ? .timingCurve(0.2, 0.8, 0.2, 1, duration: 0.03)
                                 : .timingCurve(0.2, 0.8, 0.2, 1, duration: 0.26), value: flash)
        }
        .frame(width: spec.size, height: spec.size)
        .allowsHitTesting(false)
    }
}

/// One slot's label on the ring.
struct SlotLabel: View {
    let slot: Slot
    let look: WheelLook
    let width: Double
    let hot: Bool

    var body: some View {
        let skin = look.skin
        let color = hot ? look.hotText : look.text
        let title = Text(slot.label)
            .font(look.font(skin.labelSize))
            .tracking(skin.letterSpacing * skin.labelSize)
        VStack(spacing: 3) {
            if skin.labels == .icons {
                Image(systemName: slot.symbol)
                    .font(.system(size: skin.labelSize + 4, weight: WheelLook.weight(skin.labelWeight)))
            }
            title
                .textCase(skin.uppercase ? .uppercase : nil)
                .lineLimit(2)
                .minimumScaleFactor(0.6)
                .multilineTextAlignment(.center)
        }
        .frame(width: width)
        .foregroundStyle(color)
    }
}

/// A preset's label: Orbit's outer ring and Bloom's petals.
struct PresetLabel: View {
    let text: String
    let look: WheelLook
    let color: Color
    let width: Double

    var body: some View {
        let skin = look.skin
        Text(text)
            .font(look.font(max(9, skin.labelSize - 2.5)))
            .tracking(skin.letterSpacing * skin.labelSize * 0.8)
            .textCase(skin.uppercase ? .uppercase : nil)
            .lineLimit(1)
            .minimumScaleFactor(0.6)
            .frame(width: width)
            .foregroundStyle(color)
    }
}

/// One of Bloom's presets: a pill in a pill.
struct Petal: View {
    let text: String
    let look: WheelLook
    let hot: Bool
    let flash: Bool

    var body: some View {
        ZStack {
            Capsule().fill(hot ? look.accent : look.ground2)
            Capsule().fill(Color.white)
                .opacity(flash && hot ? 0.6 : 0)
                .animation(flash ? .timingCurve(0.2, 0.8, 0.2, 1, duration: 0.03)
                                 : .timingCurve(0.2, 0.8, 0.2, 1, duration: 0.26), value: flash)
            PresetLabel(text: text, look: look, color: hot ? look.accentText : look.text, width: 80)
        }
        .frame(width: 96, height: 34)
        .padding(3)
        .background(Capsule().fill(look.text.opacity(0.06)))
        .overlay(Capsule().strokeBorder(look.text.opacity(0.10), lineWidth: 1))
        .shadow(color: hot ? look.glow : look.shadow.opacity(0.6), radius: hot ? look.skin.glowRadius : 10, y: hot ? 0 : 6)
    }
}

/// The line from a slot out to its open petals.
struct StemShape: Shape {
    var angle: Double
    var inner: Double
    var outer: Double

    func path(in rect: CGRect) -> Path {
        let c = CGPoint(x: rect.midX, y: rect.midY)
        var p = Path()
        p.move(to: polar(c, inner, angle))
        p.addLine(to: polar(c, outer, angle))
        return p
    }
}

/// The whole wheel.
package struct WheelView: View {
    @ObservedObject package var model: WheelModel

    package init(model: WheelModel) {
        _model = ObservedObject(wrappedValue: model)
    }

    package var body: some View {
        let spec = model.spec
        let look = model.look
        ZStack {
            WheelBase(spec: spec, look: look, presets: (0..<spec.count).map { model.shownPresets(at: $0).count })
            highlights(spec, look)
            hub(spec, look)
            ringLabels(spec, look)
            if spec.style == .orbit { presetLabels(spec, look) }
            if spec.style == .bloom { petals(spec, look) }
            if let i = model.lastIndex {
                let track = (spec.contentOuter + spec.rim - look.skin.bezelWidth) / 2 + 1
                Circle()
                    .fill(look.accent)
                    .frame(width: 6, height: 6)
                    .position(spec.point(radius: facet(track, spec.angle(of: i), span: look.facetSpan(spec)),
                                         angle: spec.angle(of: i)))
                    .opacity(model.pick == nil ? 1 : 0.5)
            }
        }
        .frame(width: spec.size, height: spec.size)
        .scaleEffect(model.shown || look.reduceMotion ? 1 : (model.committed ? 1.05 : 0.9))
        .opacity(model.shown ? 1 : 0)
    }

    @ViewBuilder
    private func highlights(_ spec: WheelSpec, _ look: WheelLook) -> some View {
        let pick = model.pick
        let onPreset = pick?.preset != nil
        let orbitPreset = spec.style == .orbit && onPreset
        RingHighlight(spec: spec, look: look, inner: spec.hub, outer: spec.ringOuter, width: spec.span,
                      strong: !orbitPreset, flash: model.flash)
            .rotationEffect(.degrees(model.ringAngle))
            // Glides between slots; snaps into place when it first appears.
            .animation(model.ringGlides ? look.spring : nil, value: model.ringAngle)
            .opacity(pick == nil ? 0 : 1)
        if spec.style == .orbit {
            let centre = pick.map { WheelAngle.diff(spec.angle(of: $0.slot), model.presetAngle) } ?? 0
            RingHighlight(spec: spec, look: look, inner: spec.presetInner, outer: spec.presetOuter,
                          width: model.presetWidth, strong: true, flash: model.flash, centre: centre)
                .rotationEffect(.degrees(model.presetAngle))
                .animation(model.presetGlides ? look.spring : nil, value: model.presetAngle)
                .opacity(orbitPreset ? 1 : 0)
        }
    }

    private func hub(_ spec: WheelSpec, _ look: WheelLook) -> some View {
        let sides = look.sides(spec)
        let base = model.hoveredBase
        let preset = model.hoveredPreset
        return ZStack {
            RingShape(inner: 0, outer: spec.hub, sides: sides)
                .fill(LinearGradient(colors: [look.bezel.opacity(0.8), look.bezel2], startPoint: .top, endPoint: .bottom))
                .shadow(color: look.shadow, radius: 8, y: 4)
            if look.skin.material == .glass {
                // Frosted, so the hub's text reads against the rim's light
                // gradient and whatever is behind the wheel.
                RingShape(inner: 0, outer: spec.hub - 5, sides: sides).fill(.regularMaterial)
            }
            RingShape(inner: 0, outer: spec.hub - 5, sides: sides)
                .fill(RadialGradient(colors: [look.ground2, look.ground], center: UnitPoint(x: 0.5, y: 0.3),
                                     startRadius: 0, endRadius: spec.hub))
            VStack(spacing: 2) {
                if model.loading {
                    ProgressView().controlSize(.small)
                } else if let base {
                    Text(base.label)
                        .font(look.font(12, weight: 600))
                        .foregroundStyle(preset == nil ? look.accent : look.text)
                        .lineLimit(2)
                        .minimumScaleFactor(0.7)
                        .multilineTextAlignment(.center)
                    if let preset {
                        Text(preset.label)
                            .font(look.font(11, weight: 600))
                            .foregroundStyle(look.accent)
                            .lineLimit(1)
                            .minimumScaleFactor(0.7)
                    }
                } else {
                    Text(model.title)
                        .font(look.font(12, weight: 600))
                        .foregroundStyle(look.text)
                        .lineLimit(2)
                        .minimumScaleFactor(0.7)
                        .multilineTextAlignment(.center)
                    Text(model.message ?? model.subtitle)
                        .font(look.font(10, weight: 400))
                        .foregroundStyle(look.textDim)
                        .lineLimit(2)
                        .multilineTextAlignment(.center)
                }
            }
            .frame(width: spec.hub * 2 - 20)
        }
        .frame(width: spec.hub * 2, height: spec.hub * 2)
        .position(x: spec.center, y: spec.center)
    }

    private func ringLabels(_ spec: WheelSpec, _ look: WheelLook) -> some View {
        let chord = spec.count > 1 ? 2 * spec.labelRadius * sin(spec.span / 2 * .pi / 180) : 120
        let width = min(max(chord - 12, 40), 96)
        let stagger = look.reduceMotion ? 0 : look.skin.stagger / 1000
        return ForEach(Array(model.positions.enumerated()), id: \.offset) { i, slot in
            let a = spec.angle(of: i) * .pi / 180
            let hot = model.pick?.slot == i && (model.pick?.preset == nil || spec.style != .orbit)
            let parent = model.pick?.slot == i
            let push: Double = (hot ? 5 : 0) + (model.shown || look.reduceMotion ? 0 : -10)
            let opacity: Double = {
                guard let p = model.pick else { return 1 }
                if spec.style == .orbit && p.preset != nil { return parent ? 0.9 : 0.35 }
                if spec.style == .bloom && p.preset != nil { return parent ? 1 : 0.35 }
                // Dimmed, but still easy to read on see-through skins.
                return hot ? 1 : 0.72
            }()
            SlotLabel(slot: slot, look: look, width: width, hot: hot || (parent && spec.style == .bloom))
                .scaleEffect(hot ? 1.07 : 1)
                .offset(x: push * sin(a), y: -push * cos(a))
                .opacity(model.shown ? opacity : 0)
                .animation(look.spring.delay(model.shown ? Double(i) * stagger : 0), value: model.shown)
                .animation(look.spring, value: hot)
                .position(spec.point(radius: spec.labelRadius, angle: spec.angle(of: i)))
                // Slots usually arrive a moment after the wheel opens.
                .transition((look.reduceMotion ? AnyTransition.opacity
                                               : AnyTransition.opacity.combined(with: .scale(scale: 0.85)))
                    .animation(look.spring.delay(Double(i) * stagger)))
        }
    }

    private func presetLabels(_ spec: WheelSpec, _ look: WheelLook) -> some View {
        let width = min(2 * spec.presetRadius * sin(spec.span / 4 * .pi / 180) - 6, 84)
        let near = 0.28 + 0.6 * model.proximity
        return ForEach(Array(model.positions.enumerated()), id: \.offset) { i, _ in
            let presets = model.shownPresets(at: i)
            ForEach(Array(presets.enumerated()), id: \.offset) { v, preset in
                let hot = model.pick == WheelPick(slot: i, preset: v)
                let opacity: Double = {
                    guard model.shown else { return 0 }
                    guard let p = model.pick, p.preset != nil else { return near }
                    if hot { return 1 }
                    return p.slot == i ? 0.85 : 0.45
                }()
                let angle = spec.presetAngle(slot: i, preset: v, shown: presets.count)
                let fs = look.facetSpan(spec)
                let radius = (facet(spec.presetInner, angle, span: fs) + facet(spec.presetOuter, angle, span: fs)) / 2
                PresetLabel(text: preset.label, look: look, color: hot ? look.hotText : look.text,
                            width: presets.count == 1 ? width * 1.8 : width)
                    .scaleEffect(hot ? 1.08 : 1)
                    .opacity(opacity)
                    .animation(look.spring, value: hot)
                    .animation(.timingCurve(0.2, 0.8, 0.2, 1, duration: 0.12), value: model.proximity)
                    .position(spec.point(radius: radius, angle: angle))
            }
        }
    }

    private func petals(_ spec: WheelSpec, _ look: WheelLook) -> some View {
        let slot = model.bloom ?? model.lastBloom
        let presets = slot < spec.count ? model.shownPresets(at: slot) : []
        let open = model.bloom != nil && model.shown
        return ZStack {
            StemShape(angle: spec.angle(of: slot), inner: spec.ringOuter + 6, outer: spec.petalRadius - 24)
                .stroke(look.accent.opacity(0.7), lineWidth: 1)
                .opacity(open && model.pick?.slot == slot ? 1 : 0)
            ForEach(0..<WheelSpec.maxPetals, id: \.self) { k in
                let has = k < presets.count
                let shownHere = open && has
                let hot = model.pick == WheelPick(slot: slot, preset: k)
                // Out from the ring with a spring; with Reduce Motion they
                // only fade, in place.
                let still = look.reduceMotion
                let scale: Double = still ? 1 : (shownHere ? (hot ? 1.07 : 1) : 0.5)
                let radius = shownHere || still ? spec.petalRadius : spec.ringOuter - 8
                Petal(text: has ? presets[k].label : "", look: look, hot: hot, flash: model.flash)
                    .scaleEffect(scale)
                    .opacity(shownHere ? 1 : 0)
                    .position(spec.point(radius: radius, angle: spec.petalAngle(slot: slot, petal: has ? k : 0)))
                    .animation(look.spring.delay(shownHere && !still ? Double(k) * 0.03 : 0), value: model.bloom)
            }
        }
        .frame(width: spec.size, height: spec.size)
    }
}
