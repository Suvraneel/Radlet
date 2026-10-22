import Foundation

/// How the wheel is laid out. Every style keeps the same slots in the same
/// places, so switching never means relearning where things are; the
/// advanced styles add a second level for each slot's presets.
public enum WheelStyle: String, CaseIterable, Identifiable, Sendable {
    /// One ring of slots.
    case dial
    /// The dial plus an outer ring holding two presets behind each slot.
    case orbit
    /// The dial; resting on a slot, or flicking past it, fans out its presets.
    case bloom

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .dial: return "Dial"
        case .orbit: return "Orbit"
        case .bloom: return "Bloom"
        }
    }

    public var summary: String {
        switch self {
        case .dial: return "One ring. Point and let go."
        case .orbit: return "Presets wait on an outer ring: push further for more."
        case .bloom: return "Presets fan out when you rest on a slot or flick past it."
        }
    }

    /// Orbit and Bloom are opt-in.
    public var isAdvanced: Bool { self != .dial }
}

/// Angles, in degrees, run clockwise from 12 o'clock.
public enum WheelAngle {
    /// The angle of an offset from the centre, with y pointing down.
    public static func of(dx: Double, dy: Double) -> Double {
        let a = atan2(dx, -dy) * 180 / .pi
        return a < 0 ? a + 360 : a
    }

    /// `a` minus `b`, the short way round: -180 up to 180.
    public static func diff(_ a: Double, _ b: Double) -> Double {
        var d = (a - b).truncatingRemainder(dividingBy: 360)
        if d > 180 { d -= 360 }
        if d <= -180 { d += 360 }
        return d
    }

    /// The angle to animate to so a highlight at `current` (which may have
    /// wound past 360) reaches `target` the short way.
    public static func turn(from current: Double, to target: Double) -> Double {
        current + diff(target, current)
    }

    /// The slot under angle `a` on a ring of `count`, slot 0 centred on
    /// 12 o'clock. While `current` is under the pointer it keeps it until
    /// the pointer is `hysteresis` degrees past its edge, so a hand
    /// resting on a border doesn't flicker between two slots.
    public static func slot(at a: Double, count: Int, current: Int?, hysteresis: Double) -> Int {
        guard count > 1, a.isFinite else { return current ?? 0 }
        let span = 360 / Double(count)
        var shifted = (a + span / 2).truncatingRemainder(dividingBy: 360)
        if shifted < 0 { shifted += 360 }
        let i = min(count - 1, Int(shifted / span))
        if let current, current != i, current >= 0, current < count, hysteresis > 0,
           abs(diff(a, Double(current) * span)) < span / 2 + hysteresis {
            return current
        }
        return i
    }
}

/// Sizes for one style, in points from the centre.
public struct WheelSpec: Equatable, Sendable {
    public let style: WheelStyle
    /// Segments on the ring.
    public let count: Int
    /// The centre disc. Letting go inside it cancels.
    public let hub: Double
    /// Outer edge of the ring of slots.
    public let ringOuter: Double
    /// Orbit's outer ring of presets (both 0 for the other styles).
    public let presetInner: Double
    public let presetOuter: Double
    /// Edge of the whole disc, bezel included.
    public let rim: Double
    /// Bloom: where the presets' centres sit.
    public let petalRadius: Double
    /// Side of the square panel, with room for glow and Bloom's presets.
    public let size: Double

    public static let slotCounts = [6, 8, 10, 12]
    public static let defaultCount = 8
    /// Degrees between Bloom's presets.
    public static let petalStep = 28.0
    public static let maxPetals = 4
    public static let orbitPresets = 2

    public init(style: WheelStyle, count: Int) {
        self.style = style
        self.count = max(count, 0)
        hub = 62
        switch style {
        case .dial:
            ringOuter = 164
            presetInner = 0
            presetOuter = 0
            rim = 186
            petalRadius = 0
            size = 420
        case .orbit:
            ringOuter = 140
            presetInner = 146
            presetOuter = 208
            rim = 228
            petalRadius = 0
            size = 500
        case .bloom:
            ringOuter = 140
            presetInner = 0
            presetOuter = 0
            rim = 162
            petalRadius = 214
            size = 540
        }
    }

    public var span: Double { 360 / Double(max(count, 1)) }
    public var center: Double { size / 2 }
    public var labelRadius: Double { (hub + ringOuter) / 2 }
    public var presetRadius: Double { (presetInner + presetOuter) / 2 }
    /// The outermost ring of slots or presets: tick marks sit beyond it.
    public var contentOuter: Double { style == .orbit ? presetOuter : ringOuter }

    /// Presets a slot with `available` of them shows in this style.
    public func shownPresets(_ available: Int) -> Int {
        switch style {
        case .dial: return 0
        case .orbit: return min(available, WheelSpec.orbitPresets)
        case .bloom: return min(available, WheelSpec.maxPetals)
        }
    }

    /// The middle of slot `i`.
    public func angle(of i: Int) -> Double { Double(i) * span }

    /// Orbit: the middle of preset `v` of slot `i` when the slot shows
    /// `shown` presets. A lone preset spans the whole slot.
    public func presetAngle(slot i: Int, preset v: Int, shown: Int) -> Double {
        shown <= 1 ? angle(of: i) : angle(of: i) + (v == 0 ? -span / 4 : span / 4)
    }

    /// Bloom: the first preset points straight out from its slot, so a
    /// flick picks it; the others alternate right and left of it.
    public static func petalOffset(_ k: Int) -> Double {
        guard k > 0 else { return 0 }
        return (k % 2 == 1 ? 1 : -1) * Double((k + 1) / 2) * petalStep
    }

    public func petalAngle(slot i: Int, petal k: Int) -> Double { angle(of: i) + WheelSpec.petalOffset(k) }

    /// A point at radius `r` and angle `a`, y down, in the panel's
    /// coordinates (centre at `center`, `center`).
    public func point(radius r: Double, angle a: Double) -> CGPoint {
        let rad = a * .pi / 180
        return CGPoint(x: center + r * sin(rad), y: center - r * cos(rad))
    }
}

/// Where slots go: favourites on the four fastest directions first.
public enum WheelPlacement {
    /// Positions in the order the most important slots take them: up,
    /// right, down and left, then the diagonals, then the rest, each group
    /// clockwise from the top.
    public static func order(count: Int) -> [Int] {
        guard count > 0 else { return [] }
        func offAxis(_ i: Int) -> Double {
            let a = (Double(i) * 360 / Double(count)).truncatingRemainder(dividingBy: 90)
            return min(a, 90 - a)
        }
        return (0..<count).sorted { a, b in
            let da = offAxis(a), db = offAxis(b)
            return abs(da - db) > 0.001 ? da < db : a < b
        }
    }

    /// Lays out `items`, most important first, on a ring of `items.count`.
    public static func arrange<T>(_ items: [T]) -> [T] {
        let order = order(count: items.count)
        var out = items
        for (k, item) in items.enumerated() { out[order[k]] = item }
        return out
    }
}

/// Which slot, and which of its presets, the pointer is on.
public struct WheelPick: Equatable, Sendable {
    public var slot: Int
    /// A preset of the slot, or nil for the slot itself.
    public var preset: Int?

    public init(slot: Int, preset: Int? = nil) {
        self.slot = slot
        self.preset = preset
    }
}

/// A few picks that show off a style's motion, for previews: round the
/// ring, and for Orbit and Bloom out to a slot's presets and back.
public enum WheelTour {
    /// `presets` is how many presets each position offers.
    public static func picks(spec: WheelSpec, presets: [Int]) -> [WheelPick] {
        guard spec.count > 2 else { return [] }
        let right = WheelPlacement.order(count: spec.count)[1]
        let withPresets = (0..<spec.count).first { $0 < presets.count && spec.shownPresets(presets[$0]) > 0 }
        guard spec.style != .dial, let s = withPresets else {
            return [WheelPick(slot: 0), WheelPick(slot: right), WheelPick(slot: 1)]
        }
        var tour = [WheelPick(slot: s)]
        for v in 0..<min(spec.shownPresets(presets[s]), 2) { tour.append(WheelPick(slot: s, preset: v)) }
        tour.append(WheelPick(slot: s == right ? 0 : right))
        return tour
    }
}

/// Follows the pointer for one wheel. Pure, so every rule can be tested:
/// the dead zone in the centre, hysteresis at every border, Orbit's outer
/// ring and Bloom's rest-or-flick petals.
public struct WheelTracker: Sendable {
    public let spec: WheelSpec
    /// Presets each position offers (before the style's limit).
    public let presets: [Int]
    /// Positions that hold a slot.
    public let filled: [Bool]
    /// Degrees past a border before the pick moves on. 0 turns it off.
    public var hysteresis: Double = 4
    /// Bloom: seconds resting on a slot before its presets fan out.
    public var dwell: Double = 0.24

    public private(set) var pick: WheelPick?
    /// Bloom: the slot whose presets are out.
    public private(set) var bloom: Int?
    /// Distance from the centre at the last move.
    public private(set) var distance: Double = 0

    private var cell: Int?
    private var dwellSlot: Int?
    private var dwellStart: Double = 0

    public init(spec: WheelSpec, presets: [Int], filled: [Bool]? = nil) {
        self.spec = spec
        self.presets = (0..<spec.count).map { $0 < presets.count ? spec.shownPresets(presets[$0]) : 0 }
        let f = filled ?? []
        self.filled = (0..<spec.count).map { $0 < f.count ? f[$0] : (filled == nil) }
    }

    /// When `tick` should next run, while Bloom waits to fan out.
    public var dwellDeadline: Double? { dwellSlot == nil ? nil : dwellStart + dwell }

    /// The pointer is at (`dx`, `dy`) from the centre, y down, at `time`
    /// seconds. True when the pick or the open petals changed.
    @discardableResult
    public mutating func move(dx: Double, dy: Double, at time: Double) -> Bool {
        let before = (pick, bloom)
        let d = (dx * dx + dy * dy).squareRoot()
        distance = d
        let a = WheelAngle.of(dx: dx, dy: dy)
        // Leaving the centre takes a little more than coming back.
        let dead = pick == nil ? spec.hub - 2 : spec.hub - 10
        if spec.count == 0 || d <= dead {
            clear()
        } else {
            switch spec.style {
            case .dial:
                pick = ring(a)
                cell = nil
            case .orbit:
                orbit(d, a)
            case .bloom:
                bloomMove(d, a, time)
            }
        }
        return before.0 != pick || before.1 != bloom
    }

    /// Bloom: fans out the presets once the pointer has rested on a slot
    /// for `dwell`. True when it did.
    @discardableResult
    public mutating func tick(at time: Double) -> Bool {
        guard let s = dwellSlot, pick == WheelPick(slot: s), bloom != s, time - dwellStart >= dwell - 0.0001 else {
            return false
        }
        bloom = s
        dwellSlot = nil
        return true
    }

    /// Keyboard selection. `open` opens that slot's petals (Bloom only),
    /// for previews that show a wheel mid-gesture.
    public mutating func select(_ newPick: WheelPick?, open: Int? = nil) {
        pick = newPick
        bloom = spec.style == .bloom ? open.flatMap { (0..<spec.count).contains($0) ? $0 : nil } : nil
        cell = nil
        dwellSlot = nil
    }

    public mutating func reset() {
        clear()
        distance = 0
    }

    private mutating func clear() {
        pick = nil
        bloom = nil
        cell = nil
        dwellSlot = nil
    }

    private func ring(_ a: Double) -> WheelPick? {
        let i = WheelAngle.slot(at: a, count: spec.count, current: pick?.slot, hysteresis: hysteresis)
        return filled[i] ? WheelPick(slot: i) : nil
    }

    private mutating func orbit(_ d: Double, _ a: Double) {
        let outside = pick?.preset != nil && hysteresis > 0 ? spec.ringOuter - 4 : spec.ringOuter + 6
        guard d > outside else {
            pick = ring(a)
            cell = nil
            return
        }
        // Two cells behind each slot: the left half holds preset 0, the
        // right half preset 1.
        let cells = 2 * spec.count
        let half = spec.span / 2
        var shifted = (a + half / 2).truncatingRemainder(dividingBy: 360)
        if shifted < 0 { shifted += 360 }
        // Pushing out from the ring keeps the ring's slot: start from its
        // cell on the pointer's side, so the border stays sticky.
        var current = cell
        if current == nil, let p = pick {
            current = 2 * p.slot + (WheelAngle.diff(a, spec.angle(of: p.slot)) < 0 ? 0 : 1)
        }
        let j = WheelAngle.slot(at: shifted, count: cells, current: current, hysteresis: hysteresis * 0.75)
        cell = j
        let slot = j / 2
        guard filled[slot] else {
            pick = nil
            return
        }
        switch presets[slot] {
        case 0: pick = WheelPick(slot: slot)
        case 1: pick = WheelPick(slot: slot, preset: 0)
        default: pick = WheelPick(slot: slot, preset: j % 2)
        }
    }

    private mutating func bloomMove(_ d: Double, _ a: Double, _ time: Double) {
        cell = nil
        let onPetals = pick?.preset != nil
        let line = onPetals && hysteresis > 0 ? spec.ringOuter + 10 : spec.ringOuter + 22
        if d <= line {
            let previous = pick
            guard let p = ring(a) else {
                clear()
                return
            }
            pick = p
            if p.slot != previous?.slot || previous?.preset != nil {
                if let b = bloom, b != p.slot { bloom = nil }
                if bloom != p.slot && presets[p.slot] > 0 {
                    dwellSlot = p.slot
                    dwellStart = time
                } else {
                    dwellSlot = nil
                }
            }
            return
        }
        dwellSlot = nil
        let under = WheelAngle.slot(at: a, count: spec.count, current: bloom ?? pick?.slot, hysteresis: hysteresis)
        var open: Int
        if let b = bloom {
            open = b
        } else {
            guard filled[under] else {
                pick = nil
                return
            }
            guard presets[under] > 0 else {
                pick = WheelPick(slot: under)
                return
            }
            open = under   // a flick: no need to wait
        }
        var (k, gap) = petal(open, a, current: bloom == open ? pick?.preset : nil)
        if gap > WheelSpec.petalStep / 2 + 10 && under != open {
            // Swept well past the fan: follow the pointer to the next slot.
            guard filled[under] else {
                bloom = nil
                pick = nil
                return
            }
            guard presets[under] > 0 else {
                bloom = nil
                pick = WheelPick(slot: under)
                return
            }
            open = under
            (k, gap) = petal(open, a, current: nil)
        }
        bloom = open
        pick = WheelPick(slot: open, preset: k)
    }

    /// The petal of `slot` nearest angle `a`, and how far off it is.
    private func petal(_ slot: Int, _ a: Double, current: Int?) -> (Int, Double) {
        let off = WheelAngle.diff(a, spec.angle(of: slot))
        var best = 0
        var gap = Double.infinity
        for k in 0..<presets[slot] {
            let g = abs(off - WheelSpec.petalOffset(k))
            if g < gap {
                gap = g
                best = k
            }
        }
        if let current, current < presets[slot], current != best, hysteresis > 0,
           abs(off - WheelSpec.petalOffset(current)) < WheelSpec.petalStep / 2 + 3 {
            return (current, 0)
        }
        return (best, gap)
    }
}
