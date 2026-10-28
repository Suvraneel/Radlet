import Foundation

/// How the wheel is laid out. Every style keeps the same slots in the same
/// places, so switching never means relearning where things are; the
/// advanced styles add a second level for each slot's presets.
public enum WheelStyle: String, CaseIterable, Identifiable, Sendable {
    /// One ring of slots.
    case dial
    /// The dial plus an outer ring holding two presets behind each slot;
    /// pushing out into a slot's ring widens it to show all of them.
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
    /// Side of the square drawing, with room for glow and Bloom's presets.
    public let size: Double

    public static let slotCounts = [6, 8, 10, 12]
    public static let defaultCount = 8
    /// Degrees between Bloom's presets.
    public static let petalStep = 30.0
    /// The most presets a slot shows, in either advanced style.
    public static let maxPresets = 4
    /// Orbit: presets behind each slot until the pointer pushes into it.
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
    /// Room around the drawing that still belongs to the wheel, so a hand
    /// that overshoots a slot or a preset still lets go on the wheel and
    /// not on whatever is behind it.
    public static let panelMargin = 80.0
    /// Side of the app's panel: the drawing in the middle, the margin round it.
    public var panelSize: Double { size + 2 * WheelSpec.panelMargin }
    public var labelRadius: Double { (hub + ringOuter) / 2 }
    public var presetRadius: Double { (presetInner + presetOuter) / 2 }
    /// The outermost ring of slots or presets: tick marks sit beyond it.
    public var contentOuter: Double { style == .orbit ? presetOuter : ringOuter }

    /// Presets a slot with `available` of them can show in this style
    /// (Orbit shows the first `orbitPresets` until its ring widens, and on
    /// a one-slot wheel there's only room for those two).
    public func shownPresets(_ available: Int) -> Int {
        switch style {
        case .dial: return 0
        case .orbit: return min(available, WheelSpec.maxPresets, 2 * max(count, 1))
        case .bloom: return min(available, WheelSpec.maxPresets)
        }
    }

    /// The middle of slot `i`.
    public func angle(of i: Int) -> Double { Double(i) * span }

    /// Orbit: where preset `v` of a slot with `shown` presets sits, in
    /// degrees from the slot's middle, as (start, end). Each takes half a
    /// slot: the first two fill the slot itself, left then right, and when
    /// the ring widens the third and fourth spread over the neighbours,
    /// right then left, so nothing under the pointer moves. A lone preset
    /// spans the whole slot.
    public func presetCell(_ v: Int, shown: Int) -> (start: Double, end: Double) {
        let h = span / 2
        if shown <= 1 { return (-h, h) }
        switch v {
        case 0: return (-h, 0)
        case 1: return (0, h)
        case 2: return (h, 2 * h)
        default: return (-2 * h, -h)
        }
    }

    /// Orbit: the middle of preset `v` of slot `i`.
    public func presetAngle(slot i: Int, preset v: Int, shown: Int) -> Double {
        let c = presetCell(v, shown: shown)
        return angle(of: i) + (c.start + c.end) / 2
    }

    /// Orbit: the preset cells to draw, in degrees on the wheel, given how
    /// many presets each position has (`presets`, before the two-at-rest
    /// limit) and which slot's ring is widened. Every slot shows its first
    /// two; the widened slot shows all of its own and hides any of its
    /// neighbours' cells it covers.
    public func orbitCells(presets: [Int], focus: Int?) -> [OrbitCell] {
        func cells(_ i: Int, limit: Int) -> [OrbitCell] {
            let n = i < presets.count ? shownPresets(presets[i]) : 0
            return (0..<min(n, limit)).map { v in
                let c = presetCell(v, shown: n)
                return OrbitCell(slot: i, preset: v, start: angle(of: i) + c.start, end: angle(of: i) + c.end)
            }
        }
        guard let f = focus, (0..<count).contains(f) else {
            return (0..<count).flatMap { cells($0, limit: WheelSpec.orbitPresets) }
        }
        let wide = cells(f, limit: WheelSpec.maxPresets)
        // The widened ring is one run of cells: measure the others from its start.
        guard let low = wide.map(\.start).min(), let high = wide.map(\.end).max() else { return [] }
        let length = high - low
        let others = (0..<count).filter { $0 != f }.flatMap { cells($0, limit: WheelSpec.orbitPresets) }
        // On a two- or three-slot wheel the widened ring can wrap most of the way round.
        guard length < 359.5 else { return wide }
        let kept = others.compactMap { c -> OrbitCell? in
            let width = c.end - c.start
            var lo = (c.start - low).truncatingRemainder(dividingBy: 360)
            if lo < 0 { lo += 360 }
            // Starting past the run but wrapping round to its start: measure it from before.
            if lo > length && lo + width > 360 { lo -= 360 }
            let hi = lo + width
            if hi <= 0.5 || lo >= length - 0.5 { return c }                // clear of it
            if lo >= -0.5 && hi <= length + 0.5 { return nil }               // covered
            // Half under it (a lone preset spans its whole slot): keep the rest.
            return lo < 0 ? OrbitCell(slot: c.slot, preset: c.preset, start: c.start, end: c.start - lo)
                          : OrbitCell(slot: c.slot, preset: c.preset, start: c.end - (hi - length), end: c.end)
        }
        return (kept + wide).sorted { ($0.slot, $0.preset) < ($1.slot, $1.preset) }
    }

    /// Bloom: where preset `k` of `n` sits, in degrees from its slot. The
    /// fan is centred on the slot, a step apart, and the presets take the
    /// places nearest the middle first, right before left, so the first
    /// is straight out (or just right of it) and a flick picks it.
    public static func petalOffset(_ k: Int, of n: Int) -> Double {
        guard n > 1 else { return 0 }
        let middle = Double(n - 1) / 2
        var places: [Double] = []
        for j in 0..<n { places.append((Double(j) - middle) * petalStep) }
        places.sort { (a: Double, b: Double) -> Bool in
            let da = abs(a), db = abs(b)
            return da != db ? da < db : a > b
        }
        return places[min(max(k, 0), n - 1)]
    }

    public func petalAngle(slot i: Int, petal k: Int, of n: Int) -> Double {
        angle(of: i) + WheelSpec.petalOffset(k, of: n)
    }

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

/// Orbit: one preset's cell on the outer ring, in degrees on the wheel.
public struct OrbitCell: Equatable, Hashable, Sendable {
    public let slot: Int
    public let preset: Int
    public let start: Double
    public let end: Double

    public var middle: Double { (start + end) / 2 }
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
///
/// Orbit and Bloom share one rule for a slot's presets: once they're all
/// out (Bloom's fan, Orbit's widened ring), they stay out wherever the
/// pointer goes, overshoots, the centre and (while dragging) beyond the
/// wheel included, until it rests on another slot of the inner ring. Past
/// the ends of the presets nothing is picked, so letting go there does
/// nothing.
public struct WheelTracker: Sendable {
    public let spec: WheelSpec
    /// Presets each position offers (before the style's limit).
    public let presets: [Int]
    /// Positions that hold a slot.
    public let filled: [Bool]
    /// Degrees past a border before the pick moves on. 0 turns it off.
    public var hysteresis: Double = 4
    /// Seconds resting on a slot before Bloom's presets fan out, or before
    /// open presets give way to the slot under the pointer.
    public var dwell: Double = 0.24

    public private(set) var pick: WheelPick?
    /// The slot whose presets are all out: Bloom's fan, Orbit's widened ring.
    public private(set) var focus: Int?
    /// Distance from the centre at the last move.
    public private(set) var distance: Double = 0

    private var cell: Int?
    /// The last move was out past the inner ring.
    private var outside = false
    /// What happens once the pointer has rested for `dwell`.
    private var pending: Pending?
    private var pendingSince: Double = 0

    private enum Pending: Equatable, Sendable {
        /// This slot's presets fan out (Bloom).
        case open(Int)
        /// The open presets fold away: the pointer settled on another slot
        /// with none to show, or (Orbit) on any other slot.
        case close
    }

    public init(spec: WheelSpec, presets: [Int], filled: [Bool]? = nil) {
        self.spec = spec
        self.presets = (0..<spec.count).map { $0 < presets.count ? spec.shownPresets(presets[$0]) : 0 }
        let f = filled ?? []
        self.filled = (0..<spec.count).map { $0 < f.count ? f[$0] : (filled == nil) }
    }

    /// When `tick` should next run, while presets wait to fan out or fold away.
    public var dwellDeadline: Double? { pending == nil ? nil : pendingSince + dwell }

    /// The pointer is at (`dx`, `dy`) from the centre, y down, at `time`
    /// seconds. True when the pick or the open petals changed.
    @discardableResult
    public mutating func move(dx: Double, dy: Double, at time: Double) -> Bool {
        let before = (pick, focus)
        let d = (dx * dx + dy * dy).squareRoot()
        distance = d
        let a = WheelAngle.of(dx: dx, dy: dy)
        // Leaving the centre takes a little more than coming back.
        let dead = pick == nil ? spec.hub - 2 : spec.hub - 10
        if spec.count == 0 {
            clear()
        } else if d <= dead {
            // The centre picks nothing; open presets stay out.
            pick = nil
            cell = nil
            outside = false
            pending = nil
        } else {
            switch spec.style {
            case .dial:
                pick = ring(a)
                cell = nil
            case .orbit:
                orbit(d, a, time)
            case .bloom:
                bloomMove(d, a, time)
            }
        }
        return before.0 != pick || before.1 != focus
    }

    /// Once the pointer has rested for `dwell`, fans out Bloom's presets for
    /// the slot it's on, or folds away presets it has moved off. True when
    /// the open presets changed.
    @discardableResult
    public mutating func tick(at time: Double) -> Bool {
        guard let p = pending, time - pendingSince >= dwell - 0.0001 else { return false }
        pending = nil
        switch p {
        case .open(let s):
            guard pick == WheelPick(slot: s), focus != s else { return false }
            focus = s
        case .close:
            guard focus != nil else { return false }
            focus = nil
        }
        return true
    }

    /// Keyboard selection. `open` puts that slot's presets all out (Orbit
    /// and Bloom), for previews that show a wheel mid-gesture.
    public mutating func select(_ newPick: WheelPick?, open: Int? = nil) {
        pick = newPick
        focus = spec.style.isAdvanced ? open.flatMap { (0..<spec.count).contains($0) && presets[$0] > 0 ? $0 : nil } : nil
        cell = nil
        outside = false
        pending = nil
    }

    public mutating func reset() {
        clear()
        distance = 0
    }

    /// The pointer left the wheel. Nothing is picked out there; with
    /// `keepOpen` (still dragging) open presets stay out for when it comes
    /// back.
    public mutating func leave(keepOpen: Bool) {
        guard keepOpen else {
            reset()
            return
        }
        pick = nil
        cell = nil
        pending = nil
    }

    private mutating func clear() {
        pick = nil
        focus = nil
        cell = nil
        outside = false
        pending = nil
    }

    private func ring(_ a: Double) -> WheelPick? {
        let i = WheelAngle.slot(at: a, count: spec.count, current: pick?.slot, hysteresis: hysteresis)
        return filled[i] ? WheelPick(slot: i) : nil
    }

    /// The inner ring, for Orbit and Bloom. The ring follows the pointer;
    /// presets that are all out stay out, whatever the angle, until the
    /// pointer rests on another slot.
    private mutating func innerRing(_ a: Double, _ d: Double, _ time: Double) {
        let previous = pick
        let wasOutside = outside
        outside = false
        cell = nil
        // Under an open fan the outer edge of the ring leads to its petals:
        // it stays with the fan's slot, so heading for a petal over a
        // neighbour, however slowly, never swaps the fan away.
        if spec.style == .bloom, let f = focus, d > (spec.labelRadius + spec.ringOuter) / 2, underFan(f, a) {
            pick = WheelPick(slot: f)
            pending = nil
            return
        }
        let p = ring(a)
        pick = p
        guard p != previous || wasOutside else { return }
        if let f = focus, p?.slot != f {
            let swap = spec.style == .bloom ? p.flatMap { presets[$0.slot] > 0 ? $0.slot : nil } : nil
            pending = swap.map { .open($0) } ?? .close
            pendingSince = time
        } else if spec.style == .bloom, let p, focus != p.slot, presets[p.slot] > 0 {
            pending = .open(p.slot)
            pendingSince = time
        } else {
            pending = nil
        }
    }

    private mutating func orbit(_ d: Double, _ a: Double, _ time: Double) {
        let line = outside && hysteresis > 0 ? spec.ringOuter - 4 : spec.ringOuter + 6
        guard d > line else {
            innerRing(a, d, time)
            return
        }
        let fromRing = !outside
        outside = true
        pending = nil
        // A widened ring keeps the pointer: past its ends, nothing.
        if let f = focus {
            if let v = orbitCell(f, a, current: pick?.slot == f ? pick?.preset : nil, slack: !fromRing) {
                pick = WheelPick(slot: f, preset: v)
                return
            }
            if !fromRing {
                pick = nil
                return
            }
            // Pushed out from the ring beside it: the cells there are the
            // slot's own, as drawn.
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
        // A slot with nothing behind it changes nothing that's open.
        guard filled[slot] else {
            pick = nil
            return
        }
        switch presets[slot] {
        case 0:
            pick = WheelPick(slot: slot)
        case 1:
            pick = WheelPick(slot: slot, preset: 0)
            focus = slot
        default:
            pick = WheelPick(slot: slot, preset: j % 2)
            focus = slot
        }
    }

    /// Orbit: the preset of `slot`'s widened ring under angle `a`, sticking
    /// to `current` near its edges and, with `slack` (moving along the outer
    /// ring rather than coming out from the inner one), allowing a quarter
    /// slot past the ends where no other cell is drawn; nil beyond that.
    private func orbitCell(_ slot: Int, _ a: Double, current: Int?, slack: Bool) -> Int? {
        let n = presets[slot]
        guard n > 0 else { return nil }
        let off = WheelAngle.diff(a, spec.angle(of: slot))
        let cells = (0..<n).map { spec.presetCell($0, shown: n) }
        let stick = hysteresis * 0.75
        if let current, current < n, off >= cells[current].start - stick, off <= cells[current].end + stick {
            return current
        }
        if let v = cells.firstIndex(where: { off >= $0.start && off < $0.end }) { return v }
        guard slack, !drawnElsewhere(a, focus: slot) else { return nil }
        let margin = spec.span / 4
        let low = cells.map(\.start).min()!, high = cells.map(\.end).max()!
        if off < low && off >= low - margin { return cells.firstIndex { $0.start == low } }
        if off >= high && off <= high + margin { return cells.firstIndex { $0.end == high } }
        return nil
    }

    /// Orbit: whether another slot's cell is drawn under angle `a` while
    /// `focus`'s ring is widened.
    private func drawnElsewhere(_ a: Double, focus f: Int) -> Bool {
        spec.orbitCells(presets: presets, focus: f).contains { c in
            let d = WheelAngle.diff(a, c.start)
            return c.slot != f && d >= 0 && d < c.end - c.start
        }
    }

    private mutating func bloomMove(_ d: Double, _ a: Double, _ time: Double) {
        cell = nil
        let line = outside && hysteresis > 0 ? spec.ringOuter + 10 : spec.ringOuter + 22
        guard d > line else {
            innerRing(a, d, time)
            return
        }
        outside = true
        pending = nil
        if let f = focus {
            // The open fan keeps the pointer: past its ends, nothing.
            pick = petal(f, a, current: pick?.slot == f ? pick?.preset : nil).map { WheelPick(slot: f, preset: $0) }
            return
        }
        // A flick: nothing open yet, so open the slot under the pointer at
        // its first preset.
        let under = WheelAngle.slot(at: a, count: spec.count, current: pick?.slot, hysteresis: hysteresis)
        guard filled[under] else {
            pick = nil
            return
        }
        guard presets[under] > 0 else {
            pick = WheelPick(slot: under)
            return
        }
        // Starting from the first preset: straight out picks it, and the
        // next move won't flip to another without a reason.
        focus = under
        pick = WheelPick(slot: under, preset: petal(under, a, current: 0) ?? 0)
    }

    /// Bloom: whether angle `a` is within the span of `slot`'s petals, half
    /// a step past the outermost.
    private func underFan(_ slot: Int, _ a: Double) -> Bool {
        let n = presets[slot]
        guard n > 1 else { return false }
        let off = WheelAngle.diff(a, spec.angle(of: slot))
        let offsets = (0..<n).map { WheelSpec.petalOffset($0, of: n) }
        let margin = WheelSpec.petalStep / 2
        return off >= offsets.min()! - margin && off <= offsets.max()! + margin
    }

    /// Bloom: the petal of `slot` nearest angle `a`, sticking to `current`
    /// until the pointer is past halfway to the next; nil more than half a
    /// step beyond the outermost petals (or beyond the slot's own edges, if
    /// that's further).
    private func petal(_ slot: Int, _ a: Double, current: Int?) -> Int? {
        let n = presets[slot]
        guard n > 0 else { return nil }
        let off = WheelAngle.diff(a, spec.angle(of: slot))
        let offsets = (0..<n).map { WheelSpec.petalOffset($0, of: n) }
        let margin = WheelSpec.petalStep / 2
        // At least as wide as the slot itself, so one or two presets can be
        // reached from anywhere straight out from it.
        let reach = spec.span / 2 + hysteresis
        guard off >= min(offsets.min()! - margin, -reach), off <= max(offsets.max()! + margin, reach) else { return nil }
        if let current, current < n, hysteresis > 0, abs(off - offsets[current]) < margin + 3 {
            return current
        }
        return offsets.indices.min { abs(off - offsets[$0]) < abs(off - offsets[$1]) }
    }
}
