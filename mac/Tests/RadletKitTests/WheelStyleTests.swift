import Foundation
import XCTest
@testable import RadletKit

final class WheelStyleTests: XCTestCase {
    @discardableResult
    private func move(_ t: inout WheelTracker, _ r: Double, _ degrees: Double, _ time: Double = 0) -> Bool {
        let a = degrees * .pi / 180
        return t.move(dx: r * sin(a), dy: -r * cos(a), at: time)
    }

    func testAngles() {
        XCTAssertEqual(WheelAngle.of(dx: 0, dy: -1), 0, accuracy: 1e-9)       // up
        XCTAssertEqual(WheelAngle.of(dx: 1, dy: 0), 90, accuracy: 1e-9)       // right
        XCTAssertEqual(WheelAngle.of(dx: -1, dy: 0), 270, accuracy: 1e-9)
        XCTAssertEqual(WheelAngle.diff(350, 10), -20, accuracy: 1e-9)
        XCTAssertEqual(WheelAngle.diff(10, 350), 20, accuracy: 1e-9)
        XCTAssertEqual(WheelAngle.turn(from: 350, to: 10), 370, accuracy: 1e-9)
        XCTAssertEqual(WheelAngle.turn(from: 720, to: 270), 630, accuracy: 1e-9)
        // Bloom's fan: centred on the slot, 30° apart, nearest the middle first.
        XCTAssertEqual(WheelSpec.petalOffset(0, of: 1), 0)
        XCTAssertEqual((0..<2).map { WheelSpec.petalOffset($0, of: 2) }, [15, -15])
        XCTAssertEqual((0..<3).map { WheelSpec.petalOffset($0, of: 3) }, [0, 30, -30])
        XCTAssertEqual((0..<4).map { WheelSpec.petalOffset($0, of: 4) }, [15, -15, 45, -45])
    }

    func testFavouritesTakeTheCardinalPointsFirst() {
        XCTAssertEqual(WheelPlacement.order(count: 8), [0, 2, 4, 6, 1, 3, 5, 7])
        XCTAssertEqual(WheelPlacement.order(count: 6), [0, 3, 1, 2, 4, 5])
        XCTAssertEqual(Array(WheelPlacement.order(count: 12).prefix(4)), [0, 3, 6, 9])
        XCTAssertEqual(WheelPlacement.order(count: 1), [0])
        XCTAssertEqual(WheelPlacement.order(count: 0), [])
        // The most useful slot goes up, the next right, then down and left;
        // "More…", last, takes the last diagonal.
        XCTAssertEqual(WheelPlacement.arrange(["a", "b", "c", "d", "e", "f", "g", "more"]),
                       ["a", "e", "b", "f", "c", "g", "d", "more"])
    }

    func testUsageChoosesWhatStaysButNotWhereItGoes() {
        let slots = (0..<10).map { Slot(id: "s\($0)", label: "S\($0)…", symbol: "circle", group: "tool", action: .null) }
        let laid = WheelLayout.fit(slots, max: 4, usage: ["s7": 5, "s2": 3])
        XCTAssertEqual(laid.visible.map(\.id), ["s0", "s2", "s7", WheelLayout.moreID])
        XCTAssertEqual(laid.overflow.map(\.id), ["s1", "s3", "s4", "s5", "s6", "s8", "s9"])
        let more = WheelLayout.morePresets(laid.overflow)
        XCTAssertEqual(more.map(\.id), ["s1", "s3", "s4", "s5"])
        XCTAssertEqual(more.first?.label, "S1")
        // Nothing used yet: the same as before.
        XCTAssertEqual(WheelLayout.fit(slots, max: 4, usage: [:]).visible.map(\.id), ["s0", "s1", "s2", WheelLayout.moreID])
        XCTAssertEqual(WheelLayout.fit(Array(slots.prefix(3)), max: 4, usage: ["s2": 9]).visible.map(\.id), ["s0", "s1", "s2"])
    }

    func testLessKnownFormatsWaitBehindMore() {
        func slot(_ id: String, more: Bool = false) -> Slot {
            Slot(id: id, label: id, symbol: "photo", group: "convert", action: .null, more: more)
        }
        let slots = [slot("jpeg"), slot("png"), slot("pdf"), slot("tga", more: true), slot("qoi", more: true),
                     slot("bmp", more: true)]
        // Room for all six, but the rare three still wait behind "More…".
        let laid = WheelLayout.fit(slots, max: 8, usage: [:])
        XCTAssertEqual(laid.visible.map(\.id), ["jpeg", "png", "pdf", WheelLayout.moreID])
        XCTAssertEqual(laid.overflow.map(\.id), ["tga", "qoi", "bmp"])
        // Using one puts it first behind "More…", not on the wheel.
        let used = WheelLayout.fit(slots, max: 8, usage: ["bmp": 4])
        XCTAssertEqual(used.visible.map(\.id), ["jpeg", "png", "pdf", WheelLayout.moreID])
        XCTAssertEqual(used.overflow.map(\.id), ["bmp", "tga", "qoi"])
        // A small wheel: popular ones that don't fit come before the rare.
        let small = WheelLayout.fit(slots, max: 3, usage: [:])
        XCTAssertEqual(small.visible.map(\.id), ["jpeg", "png", WheelLayout.moreID])
        XCTAssertEqual(small.overflow.map(\.id), ["pdf", "tga", "qoi", "bmp"])
        // Just one rare slot: "More…" would only take its place.
        let one = WheelLayout.fit(Array(slots.prefix(4)), max: 8, usage: [:])
        XCTAssertEqual(one.visible.map(\.id), ["jpeg", "png", "pdf", "tga"])
        // Only rare ones: shown as usual.
        XCTAssertEqual(WheelLayout.fit(Array(slots.suffix(3)), max: 8, usage: [:]).visible.count, 3)
    }

    func testSlotsSayWhenTheyWaitBehindMore() throws {
        let json = #"{"id":"to_tga","label":"TGA","symbol":"photo","group":"convert","action":{"kind":"convert"},"more":true}"#
        let slot = try JSONDecoder().decode(Slot.self, from: Data(json.utf8))
        XCTAssertTrue(slot.more)
        XCTAssertTrue(slot.unlocked.more)
        let plain = #"{"id":"to_png","label":"PNG","symbol":"photo","group":"convert","action":{"kind":"convert"}}"#
        XCTAssertFalse(try JSONDecoder().decode(Slot.self, from: Data(plain.utf8)).more)
    }

    /// The point where each label is drawn hits its own slot, for every
    /// style and size: ties drawing (y down) to hit-testing.
    func testEveryLabelHitsItsSlot() {
        for style in WheelStyle.allCases {
            for count in WheelSpec.slotCounts + [1, 3, 5] {
                let spec = WheelSpec(style: style, count: count)
                for i in 0..<count {
                    var t = WheelTracker(spec: spec, presets: Array(repeating: 2, count: count))
                    let p = spec.point(radius: spec.labelRadius, angle: spec.angle(of: i))
                    t.move(dx: Double(p.x) - spec.center, dy: Double(p.y) - spec.center, at: 0)
                    XCTAssertEqual(t.pick, WheelPick(slot: i), "\(style) slot \(i) of \(count)")
                }
                XCTAssertGreaterThan(spec.size / 2, spec.rim)
                if style == .bloom { XCTAssertGreaterThan(spec.size / 2, spec.petalRadius + 40) }
            }
        }
    }

    func testDialCentreCancelsAndBordersAreSticky() {
        var t = WheelTracker(spec: WheelSpec(style: .dial, count: 8), presets: [])
        move(&t, 20, 0)
        XCTAssertNil(t.pick)
        move(&t, 100, 0)
        XCTAssertEqual(t.pick, WheelPick(slot: 0))
        XCTAssertTrue(move(&t, 100, 90))
        XCTAssertEqual(t.pick?.slot, 2)
        XCTAssertFalse(move(&t, 100, 95))
        // 22.5° is the border between slots 0 and 1.
        move(&t, 100, 0)
        move(&t, 100, 25)
        XCTAssertEqual(t.pick?.slot, 0)
        move(&t, 100, 27)
        XCTAssertEqual(t.pick?.slot, 1)
        move(&t, 100, 20)
        XCTAssertEqual(t.pick?.slot, 1)
        move(&t, 100, 17)
        XCTAssertEqual(t.pick?.slot, 0)
        // Direction is what counts, however far out.
        move(&t, 400, 180)
        XCTAssertEqual(t.pick?.slot, 4)
        // Back in the centre: returning takes 52 points, leaving 60.
        move(&t, 55, 180)
        XCTAssertEqual(t.pick?.slot, 4)
        move(&t, 50, 180)
        XCTAssertNil(t.pick)
        move(&t, 58, 180)
        XCTAssertNil(t.pick)
        // Without hysteresis the border is exact.
        t.hysteresis = 0
        move(&t, 100, 0)
        move(&t, 100, 23)
        XCTAssertEqual(t.pick?.slot, 1)
    }

    func testEmptyPositionsPickNothing() {
        var t = WheelTracker(spec: WheelSpec(style: .dial, count: 4), presets: [], filled: [true, false, true, true])
        move(&t, 100, 90)
        XCTAssertNil(t.pick)
        move(&t, 100, 180)
        XCTAssertEqual(t.pick?.slot, 2)
        var none = WheelTracker(spec: WheelSpec(style: .orbit, count: 0), presets: [])
        move(&none, 100, 0)
        XCTAssertNil(none.pick)
    }

    func testOrbitOuterRingPicksPresets() {
        let spec = WheelSpec(style: .orbit, count: 8)
        var t = WheelTracker(spec: spec, presets: [2, 1, 0, 3, 2, 2, 2, 2])
        XCTAssertEqual(t.presets[3], 3, "Orbit can show up to four")
        move(&t, 100, 0)
        XCTAssertEqual(t.pick, WheelPick(slot: 0))
        move(&t, 180, -10)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 0))
        XCTAssertEqual(t.focus, 0)
        move(&t, 180, 10)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 1))
        // Another slot's presets are reached through the inner ring. A lone
        // preset fills both halves; none means the slot itself.
        move(&t, 100, 45)
        move(&t, 180, 35)
        XCTAssertEqual(t.pick, WheelPick(slot: 1, preset: 0))
        move(&t, 180, 55)
        XCTAssertEqual(t.pick, WheelPick(slot: 1, preset: 0))
        move(&t, 100, 90)
        move(&t, 180, 90)
        XCTAssertEqual(t.pick, WheelPick(slot: 2))
        XCTAssertEqual(t.focus, 1, "nothing behind slot 2, so slot 1's presets stay")
        move(&t, 100, 135)
        move(&t, 180, 145)
        XCTAssertEqual(t.pick, WheelPick(slot: 3, preset: 1))
        // Slot 3's widened ring now covers slot 4's left half, until the
        // pointer rests elsewhere.
        move(&t, 100, 180)
        move(&t, 180, 170)
        XCTAssertEqual(t.pick, WheelPick(slot: 3, preset: 2))
        // The line between the rings is sticky: out past 146, back in
        // below 136.
        t.reset()
        move(&t, 100, 180)
        move(&t, 143, 180)
        XCTAssertEqual(t.pick, WheelPick(slot: 4))
        move(&t, 150, 170)
        XCTAssertEqual(t.pick, WheelPick(slot: 4, preset: 0))
        move(&t, 138, 170)
        XCTAssertEqual(t.pick, WheelPick(slot: 4, preset: 0))
        move(&t, 134, 170)
        XCTAssertEqual(t.pick, WheelPick(slot: 4))
        XCTAssertEqual(spec.presetAngle(slot: 4, preset: 0, shown: 2), 180 - 11.25, accuracy: 1e-9)
        XCTAssertEqual(spec.presetAngle(slot: 4, preset: 0, shown: 1), 180, accuracy: 1e-9)
        XCTAssertEqual(spec.presetAngle(slot: 4, preset: 2, shown: 3), 180 + 33.75, accuracy: 1e-9)
        XCTAssertEqual(spec.presetAngle(slot: 4, preset: 3, shown: 4), 180 - 33.75, accuracy: 1e-9)
    }

    func testOrbitWidensToShowEveryPreset() {
        let spec = WheelSpec(style: .orbit, count: 8)
        var t = WheelTracker(spec: spec, presets: [4, 2, 0, 0, 1, 0, 0, 2])
        move(&t, 100, 0, 0)
        move(&t, 180, 10, 0.05)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 1))
        XCTAssertEqual(t.focus, 0)
        // The widened ring's third and fourth presets sit over the
        // neighbours' halves, right then left.
        move(&t, 180, 33, 0.1)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 2))
        move(&t, 180, -33, 0.15)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 3))
        // Past the ends nothing is picked (here the pointer is over the
        // neighbour's own cell), and the ring stays wide however far the
        // pointer overshoots.
        move(&t, 180, 50, 0.2)
        XCTAssertNil(t.pick)
        move(&t, 180, 60, 0.25)
        XCTAssertNil(t.pick)
        move(&t, 180, 120, 0.3)
        XCTAssertNil(t.pick)
        XCTAssertEqual(t.focus, 0)
        move(&t, 180, 40, 0.35)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 2))
        // Back on the inner ring it stays wide until the pointer rests on
        // another slot, so pushing out over a neighbour still reaches it.
        move(&t, 100, 45, 1.0)
        XCTAssertEqual(t.pick, WheelPick(slot: 1))
        XCTAssertEqual(t.focus, 0)
        XCTAssertEqual(t.dwellDeadline ?? -1, 1.24, accuracy: 1e-9)
        move(&t, 180, 30, 1.05)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 2))
        // Pushed out where the wide ring doesn't reach: that slot's own cells.
        move(&t, 100, 180, 1.1)
        move(&t, 180, 180, 1.15)
        XCTAssertEqual(t.pick, WheelPick(slot: 4, preset: 0))
        XCTAssertEqual(t.focus, 4)
        // Resting on another slot folds a wide ring back.
        move(&t, 100, 0, 1.2)
        move(&t, 180, 10, 1.25)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 1))
        move(&t, 100, 45, 1.3)
        XCTAssertTrue(t.tick(at: 1.6))
        XCTAssertNil(t.focus)
        move(&t, 180, 30, 1.65)
        XCTAssertEqual(t.pick, WheelPick(slot: 1, preset: 0), "the neighbour's own cell again")
        XCTAssertEqual(t.focus, 1)
    }

    func testOrbitCellsWidenOverTheNeighbours() {
        let spec = WheelSpec(style: .orbit, count: 8)
        let presets = [4, 2, 0, 1, 0, 0, 0, 2]
        let resting = spec.orbitCells(presets: presets, focus: nil)
        XCTAssertEqual(resting.map { [$0.slot, $0.preset] }, [[0, 0], [0, 1], [1, 0], [1, 1], [3, 0], [7, 0], [7, 1]])
        let lone = resting.first { $0.slot == 3 }!
        XCTAssertEqual(lone.start, 112.5, accuracy: 1e-9, "a lone preset spans its slot")
        XCTAssertEqual(lone.end, 157.5, accuracy: 1e-9)
        let wide = spec.orbitCells(presets: presets, focus: 0)
        XCTAssertEqual(wide.map { [$0.slot, $0.preset] },
                       [[0, 0], [0, 1], [0, 2], [0, 3], [1, 1], [3, 0], [7, 0]], "covered halves hide")
        let third = wide.first { $0.slot == 0 && $0.preset == 2 }!
        XCTAssertEqual(third.start, 22.5, accuracy: 1e-9)
        XCTAssertEqual(third.end, 45, accuracy: 1e-9)
        XCTAssertEqual(spec.orbitCells(presets: presets, focus: 1), resting, "two presets cover nothing extra")
        XCTAssertEqual(spec.orbitCells(presets: presets, focus: 99), resting)
    }

    func testBloomRestsOrFlicks() {
        let spec = WheelSpec(style: .bloom, count: 8)
        var t = WheelTracker(spec: spec, presets: [3, 0, 6, 0, 0, 0, 0, 0])
        XCTAssertEqual(t.presets[2], 4, "Bloom shows four")
        move(&t, 100, 0, 0)
        XCTAssertEqual(t.pick, WheelPick(slot: 0))
        XCTAssertNil(t.focus)
        XCTAssertEqual(t.dwellDeadline ?? -1, 0.24, accuracy: 1e-9)
        XCTAssertFalse(t.tick(at: 0.1))
        XCTAssertTrue(t.tick(at: 0.25))
        XCTAssertEqual(t.focus, 0)
        XCTAssertNil(t.dwellDeadline)
        // Straight out is the first preset; 30° right the second, left the third.
        move(&t, 214, 0, 0.3)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 0))
        move(&t, 214, 30, 0.31)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 1))
        move(&t, 214, -30, 0.32)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 2))
        // Back on the ring the petals stay out for that slot.
        move(&t, 100, 0, 0.4)
        XCTAssertEqual(t.pick, WheelPick(slot: 0))
        XCTAssertEqual(t.focus, 0)
        // On another slot they stay until the pointer rests there; this
        // one has no presets, so then they fold away.
        move(&t, 100, 45, 0.5)
        XCTAssertEqual(t.pick, WheelPick(slot: 1))
        XCTAssertEqual(t.focus, 0)
        XCTAssertTrue(t.tick(at: 0.75))
        XCTAssertNil(t.focus)
        // A flick past slot 2 opens it at once, at its first preset.
        t.reset()
        move(&t, 100, 90, 1.0)
        move(&t, 230, 90, 1.02)
        XCTAssertEqual(t.focus, 2)
        XCTAssertEqual(t.pick, WheelPick(slot: 2, preset: 0))
        // Four presets: 15° right of the slot, 15° left, 45° right, 45° left.
        move(&t, 230, 105, 1.03)
        XCTAssertEqual(t.pick, WheelPick(slot: 2, preset: 0))
        move(&t, 230, 75, 1.04)
        XCTAssertEqual(t.pick, WheelPick(slot: 2, preset: 1))
        move(&t, 230, 135, 1.05)
        XCTAssertEqual(t.pick, WheelPick(slot: 2, preset: 2))
        move(&t, 230, 45, 1.06)
        XCTAssertEqual(t.pick, WheelPick(slot: 2, preset: 3))
        // Overshooting keeps the fan: past its ends nothing is picked.
        move(&t, 230, 0, 1.1)
        XCTAssertNil(t.pick)
        move(&t, 230, 180, 1.2)
        XCTAssertNil(t.pick)
        XCTAssertEqual(t.focus, 2)
        move(&t, 230, 130, 1.25)
        XCTAssertEqual(t.pick, WheelPick(slot: 2, preset: 2))
        // The centre picks nothing, and the fan stays out.
        move(&t, 10, 180, 1.3)
        XCTAssertNil(t.pick)
        XCTAssertEqual(t.focus, 2)
        move(&t, 230, 135, 1.35)
        XCTAssertEqual(t.pick, WheelPick(slot: 2, preset: 2))
    }

    func testHeadingSlowlyForASidePetalKeepsTheFan() {
        // "More…" on the last diagonal, its neighbours with presets of their own.
        let spec = WheelSpec(style: .bloom, count: 8)
        var t = WheelTracker(spec: spec, presets: Array(repeating: 4, count: 8))
        move(&t, spec.labelRadius, 315, 0)
        XCTAssertTrue(t.tick(at: 0.3))
        XCTAssertEqual(t.focus, 7)
        // Out over the slot to the left, near the edge of the ring, resting
        // there well past the dwell: still the same fan.
        move(&t, 150, 280, 0.4)
        XCTAssertEqual(t.pick, WheelPick(slot: 7))
        XCTAssertNil(t.dwellDeadline)
        XCTAssertFalse(t.tick(at: 2))
        XCTAssertEqual(t.focus, 7)
        // The petal straight out from that neighbour is the fan's last one.
        move(&t, spec.petalRadius, 270, 2.1)
        XCTAssertEqual(t.pick, WheelPick(slot: 7, preset: 3))
        move(&t, spec.petalRadius, 0, 2.2)
        XCTAssertEqual(t.pick, WheelPick(slot: 7, preset: 2))
        // Resting on the neighbour itself, nearer the centre, still swaps.
        move(&t, spec.labelRadius, 270, 2.3)
        XCTAssertEqual(t.pick, WheelPick(slot: 6))
        XCTAssertTrue(t.tick(at: 2.6))
        XCTAssertEqual(t.focus, 6)
    }

    func testLeavingTheWheelWhileDraggingKeepsTheFan() {
        let spec = WheelSpec(style: .bloom, count: 8)
        var t = WheelTracker(spec: spec, presets: [0, 0, 0, 0, 0, 0, 0, 4])
        move(&t, spec.labelRadius, 315, 0)
        t.tick(at: 0.3)
        move(&t, spec.petalRadius, 270, 0.4)
        XCTAssertEqual(t.pick, WheelPick(slot: 7, preset: 3))
        t.leave(keepOpen: true)
        XCTAssertNil(t.pick)
        XCTAssertEqual(t.focus, 7)
        move(&t, spec.petalRadius + 30, 272, 0.6)
        XCTAssertEqual(t.pick, WheelPick(slot: 7, preset: 3), "back in, the same petal")
        t.leave(keepOpen: false)
        XCTAssertNil(t.pick)
        XCTAssertNil(t.focus)
    }

    func testThePanelHasRoomToOvershoot() {
        for style in WheelStyle.allCases {
            let spec = WheelSpec(style: style, count: 8)
            XCTAssertEqual(spec.panelSize - spec.size, 2 * WheelSpec.panelMargin)
            // Past the outermost thing drawn, there's still wheel to let go on.
            let outermost = style == .bloom ? spec.petalRadius + 51 : spec.rim
            XCTAssertGreaterThanOrEqual(spec.panelSize / 2 - outermost, 80, "\(style)")
        }
    }

    func testBloomFlickPicksTheFirstPresetOnAnEvenFan() {
        var t = WheelTracker(spec: WheelSpec(style: .bloom, count: 8), presets: [4, 2])
        move(&t, 100, 0, 0)
        move(&t, 230, 0, 0.02)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 0), "15° right, but a flick straight out picks it")
        move(&t, 230, -2, 0.03)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 0), "sticky past the middle")
        move(&t, 230, -10, 0.04)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 1))
    }

    /// Moves in a straight line from one polar point to another, a little
    /// at a time, as a hand would.
    private func glide(_ t: inout WheelTracker, from: (Double, Double), to: (Double, Double),
                       start: Double, steps: Int = 40) {
        func xy(_ p: (Double, Double)) -> (Double, Double) {
            let a = p.1 * .pi / 180
            return (p.0 * sin(a), -p.0 * cos(a))
        }
        let (x0, y0) = xy(from), (x1, y1) = xy(to)
        for i in 1...steps {
            let f = Double(i) / Double(steps)
            t.move(dx: x0 + (x1 - x0) * f, dy: y0 + (y1 - y0) * f, at: start + 0.004 * Double(i))
        }
    }

    func testBloomPetalsOverTheNeighboursCanBeReached() {
        // Every petal, on every wheel size, straight from its slot's label:
        // the way out to a side petal crosses a neighbouring slot.
        for count in WheelSpec.slotCounts {
            let spec = WheelSpec(style: .bloom, count: count)
            for k in 0..<WheelSpec.maxPresets {
                var t = WheelTracker(spec: spec, presets: Array(repeating: 4, count: count))
                move(&t, spec.labelRadius, 0, 0)
                XCTAssertTrue(t.tick(at: 0.3))
                glide(&t, from: (spec.labelRadius, 0), to: (spec.petalRadius, spec.petalAngle(slot: 0, petal: k, of: 4)), start: 0.3)
                XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: k), "\(count) slots, petal \(k)")
                XCTAssertEqual(t.focus, 0, "\(count) slots, petal \(k)")
            }
        }
    }

    func testOpenPresetsStayUntilThePointerRests() {
        let spec = WheelSpec(style: .bloom, count: 12)
        var t = WheelTracker(spec: spec, presets: [4, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3])
        move(&t, 100, 0, 0)
        t.tick(at: 0.3)
        XCTAssertEqual(t.focus, 0)
        // Round the ring, however far: the ring follows the pointer, the
        // fan stays.
        move(&t, 100, 30, 0.4)
        XCTAssertEqual(t.pick, WheelPick(slot: 1))
        XCTAssertEqual(t.dwellDeadline ?? -1, 0.64, accuracy: 1e-9)
        move(&t, 100, 120, 0.45)
        XCTAssertEqual(t.pick, WheelPick(slot: 4))
        move(&t, 100, 200, 0.5)
        XCTAssertEqual(t.pick, WheelPick(slot: 7))
        XCTAssertEqual(t.focus, 0)
        // Out again: the fan's own petal, not the slot underneath.
        move(&t, 214, 45, 0.55)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 2))
        // Resting on a slot with presets swaps them in; on one without,
        // they fold away.
        move(&t, 100, 30, 0.6)
        XCTAssertFalse(t.tick(at: 0.7))
        XCTAssertTrue(t.tick(at: 0.85))
        XCTAssertEqual(t.focus, 1)
        move(&t, 100, 60, 0.9)
        XCTAssertTrue(t.tick(at: 1.15))
        XCTAssertNil(t.focus)
        // An empty position under the fan picks nothing, and folds it away
        // after a rest.
        var gaps = WheelTracker(spec: spec, presets: [4], filled: [true, false, true])
        move(&gaps, 100, 0, 0)
        gaps.tick(at: 0.3)
        move(&gaps, 100, 30, 0.4)
        XCTAssertNil(gaps.pick)
        XCTAssertEqual(gaps.focus, 0)
        XCTAssertTrue(gaps.tick(at: 0.65))
        XCTAssertNil(gaps.focus)
    }

    func testBloomLonePresetReachesAcrossItsSlot() {
        // One preset is straight out, but the whole slot leads to it.
        for count in WheelSpec.slotCounts {
            let spec = WheelSpec(style: .bloom, count: count)
            var t = WheelTracker(spec: spec, presets: [1, 2])
            move(&t, 100, 0, 0)
            XCTAssertTrue(t.tick(at: 0.3))
            for a in stride(from: -spec.span / 2 + 1, through: spec.span / 2 - 1, by: 1) {
                move(&t, 214, a, 0.4)
                XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 0), "\(count) slots at \(a)°")
            }
        }
        // A flick off-centre picks the nearer petal at once, without a flip on the next move.
        var t = WheelTracker(spec: WheelSpec(style: .bloom, count: 8), presets: [3])
        move(&t, 100, 10, 0)
        move(&t, 230, 20, 0.02)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 1))
        XCTAssertFalse(move(&t, 230, 21, 0.03))
    }

    func testOrbitEdgeCasesFromTheReview() {
        // A one-slot wheel only has room for two presets.
        let one = WheelSpec(style: .orbit, count: 1)
        XCTAssertEqual(one.shownPresets(4), 2)
        XCTAssertEqual(WheelSpec(style: .orbit, count: 2).shownPresets(4), 4)
        XCTAssertEqual(one.orbitCells(presets: [4], focus: 0).count, 2)
        // On a two-slot wheel four presets go all the way round, covering the other slot.
        let two = WheelSpec(style: .orbit, count: 2)
        XCTAssertEqual(two.orbitCells(presets: [3, 5], focus: 1).map(\.slot), [1, 1, 1, 1])
        // Three slots: the widened ring wraps past halfway, and still hides what it covers.
        let three = WheelSpec(style: .orbit, count: 3)
        XCTAssertEqual(three.orbitCells(presets: [2, 4, 2], focus: 1).map { [$0.slot, $0.preset] },
                       [[0, 0], [1, 0], [1, 1], [1, 2], [1, 3], [2, 1]])
        // Coming out from the ring over a neighbour's visible cell picks
        // that cell, not the widened ring's slack.
        let spec = WheelSpec(style: .orbit, count: 8)
        var t = WheelTracker(spec: spec, presets: [4, 2, 0, 0, 0, 0, 0, 0])
        move(&t, 100, 0, 0)
        move(&t, 180, 10, 0.05)
        XCTAssertEqual(t.focus, 0)
        move(&t, 100, 50, 0.1)
        move(&t, 180, 52, 0.15)
        XCTAssertEqual(t.pick, WheelPick(slot: 1, preset: 1))
        XCTAssertEqual(t.focus, 1)
        // A lone preset half under a widened ring keeps its other half, and
        // that half picks it.
        let cells = spec.orbitCells(presets: [4, 1, 0, 0, 0, 0, 0, 0], focus: 0)
        let half = cells.first { $0.slot == 1 }!
        XCTAssertEqual(half.start, 45, accuracy: 1e-9)
        XCTAssertEqual(half.end, 67.5, accuracy: 1e-9)
        var u = WheelTracker(spec: spec, presets: [4, 1, 0, 0, 0, 0, 0, 0])
        move(&u, 100, 0, 0)
        move(&u, 180, 10, 0.05)
        move(&u, 100, 55, 0.1)
        move(&u, 180, 55, 0.15)
        XCTAssertEqual(u.pick, WheelPick(slot: 1, preset: 0))
        // Over a slot with nothing behind it, the widened ring stays.
        var w = WheelTracker(spec: spec, presets: [4, 0, 0, 0, 0, 0, 0, 0])
        move(&w, 100, 0, 0)
        move(&w, 180, 10, 0.05)
        move(&w, 100, 90, 0.1)
        move(&w, 180, 90, 0.15)
        XCTAssertEqual(w.pick, WheelPick(slot: 2))
        XCTAssertEqual(w.focus, 0)
        // Selecting can't open a slot with no presets.
        w.select(WheelPick(slot: 2), open: 2)
        XCTAssertNil(w.focus)
    }

    func testOrbitKeepsTheRingsSlotWhenPushingOut() {
        var t = WheelTracker(spec: WheelSpec(style: .orbit, count: 8), presets: Array(repeating: 2, count: 8))
        move(&t, 100, 0)
        move(&t, 100, 25)
        XCTAssertEqual(t.pick, WheelPick(slot: 0), "sticky on the ring")
        move(&t, 180, 25)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 1), "and still slot 0 on the outer ring")
        move(&t, 180, 30)
        XCTAssertNil(t.pick, "over the next slot's cell: nothing, rather than switching")
        // Next to a slot with nothing behind it, there's a quarter slot of slack.
        var u = WheelTracker(spec: WheelSpec(style: .orbit, count: 8), presets: [2])
        move(&u, 100, 0)
        move(&u, 180, 10)
        move(&u, 180, 30)
        XCTAssertEqual(u.pick, WheelPick(slot: 0, preset: 1), "slack past the end")
        move(&u, 180, 40)
        XCTAssertNil(u.pick, "then nothing")
    }

    func testSmallWheelsAndOddInput() {
        for style in WheelStyle.allCases {
            for count in [1, 2] {
                var t = WheelTracker(spec: WheelSpec(style: style, count: count), presets: Array(repeating: 3, count: count))
                for a in stride(from: 0.0, to: 360, by: 15) {
                    move(&t, 100, a, a)
                    XCTAssertNotNil(t.pick, "\(style) \(count) at \(a)")
                    move(&t, 230, a, a + 0.5)
                    // Past the ends of open presets the advanced styles pick nothing.
                    if style == .dial { XCTAssertNotNil(t.pick, "\(style) \(count) out at \(a)") }
                    if let p = t.pick {
                        XCTAssertLessThan(p.slot, count)
                        XCTAssertLessThan(p.preset ?? 0, 3)
                    }
                    _ = t.tick(at: a + 1)
                }
            }
        }
        var t = WheelTracker(spec: WheelSpec(style: .dial, count: 8), presets: [])
        t.move(dx: .nan, dy: .nan, at: 0)
        t.move(dx: .infinity, dy: 1, at: 0)
        XCTAssertEqual(WheelAngle.slot(at: .nan, count: 8, current: 3, hysteresis: 4), 3)
    }

    func testDuplicateIdsAreChosenByPosition() {
        let ids = ["x", "r1", "r2", "r3", "r4", "x"]
        let slots = ids.map { Slot(id: $0, label: $0, symbol: "circle", group: "recipe", action: .null) }
        let laid = WheelLayout.fit(slots, max: 4, usage: [:])
        XCTAssertEqual(laid.visible.map(\.id), ["x", "r1", "r2", WheelLayout.moreID])
        XCTAssertEqual(laid.overflow.map(\.id), ["r3", "r4", "x"])
    }

    func testKeyboardSelection() {
        var t = WheelTracker(spec: WheelSpec(style: .bloom, count: 8), presets: [2])
        t.select(WheelPick(slot: 3))
        XCTAssertEqual(t.pick, WheelPick(slot: 3))
        XCTAssertNil(t.focus)
        t.reset()
        XCTAssertNil(t.pick)
    }

    func testToursShowEachStyle() {
        let dial = WheelSpec(style: .dial, count: 8)
        XCTAssertEqual(WheelTour.picks(spec: dial, presets: [3, 3]),
                       [WheelPick(slot: 0), WheelPick(slot: 2), WheelPick(slot: 1)])
        let orbit = WheelSpec(style: .orbit, count: 8)
        XCTAssertEqual(WheelTour.picks(spec: orbit, presets: [3] + Array(repeating: 0, count: 7)),
                       [WheelPick(slot: 0), WheelPick(slot: 0, preset: 0), WheelPick(slot: 0, preset: 1), WheelPick(slot: 2)])
        XCTAssertEqual(WheelTour.picks(spec: orbit, presets: []),
                       [WheelPick(slot: 0), WheelPick(slot: 2), WheelPick(slot: 1)], "no presets: round the ring")
        let bloom = WheelSpec(style: .bloom, count: 6)
        XCTAssertEqual(WheelTour.picks(spec: bloom, presets: [0, 0, 1]),
                       [WheelPick(slot: 2), WheelPick(slot: 2, preset: 0), WheelPick(slot: 3)])
        XCTAssertEqual(WheelTour.picks(spec: WheelSpec(style: .bloom, count: 2), presets: [4, 4]), [])
    }

    func testSelectingCanOpenPresets() {
        var bloom = WheelTracker(spec: WheelSpec(style: .bloom, count: 8), presets: Array(repeating: 3, count: 8))
        bloom.select(WheelPick(slot: 1, preset: 2), open: 1)
        XCTAssertEqual(bloom.pick, WheelPick(slot: 1, preset: 2))
        XCTAssertEqual(bloom.focus, 1)
        bloom.select(WheelPick(slot: 1), open: 9)
        XCTAssertNil(bloom.focus, "out of range")
        bloom.select(WheelPick(slot: 1))
        XCTAssertNil(bloom.focus)
        var orbit = WheelTracker(spec: WheelSpec(style: .orbit, count: 8), presets: Array(repeating: 4, count: 8))
        orbit.select(WheelPick(slot: 2, preset: 3), open: 2)
        XCTAssertEqual(orbit.focus, 2, "Orbit's ring widens")
        var dial = WheelTracker(spec: WheelSpec(style: .dial, count: 8), presets: [])
        dial.select(WheelPick(slot: 1), open: 1)
        XCTAssertNil(dial.focus, "the Dial has no presets")
    }
}
