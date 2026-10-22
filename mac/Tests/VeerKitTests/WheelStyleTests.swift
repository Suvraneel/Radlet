import Foundation
import XCTest
@testable import VeerKit

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
        XCTAssertEqual(WheelSpec.petalOffset(0), 0)
        XCTAssertEqual((1...3).map(WheelSpec.petalOffset), [28, -28, 56])
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
        XCTAssertEqual(t.presets[3], 2, "Orbit shows two")
        move(&t, 100, 0)
        XCTAssertEqual(t.pick, WheelPick(slot: 0))
        move(&t, 180, -10)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 0))
        move(&t, 180, 10)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 1))
        // A lone preset fills both halves; none means the slot itself.
        move(&t, 180, 35)
        XCTAssertEqual(t.pick, WheelPick(slot: 1, preset: 0))
        move(&t, 180, 55)
        XCTAssertEqual(t.pick, WheelPick(slot: 1, preset: 0))
        move(&t, 180, 90)
        XCTAssertEqual(t.pick, WheelPick(slot: 2))
        move(&t, 180, 145)
        XCTAssertEqual(t.pick, WheelPick(slot: 3, preset: 1))
        // The line between the rings is sticky: out past 146, back in
        // below 136.
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
    }

    func testBloomRestsOrFlicks() {
        let spec = WheelSpec(style: .bloom, count: 8)
        var t = WheelTracker(spec: spec, presets: [3, 0, 6, 0, 0, 0, 0, 0])
        XCTAssertEqual(t.presets[2], 4, "Bloom shows four")
        move(&t, 100, 0, 0)
        XCTAssertEqual(t.pick, WheelPick(slot: 0))
        XCTAssertNil(t.bloom)
        XCTAssertEqual(t.dwellDeadline ?? -1, 0.24, accuracy: 1e-9)
        XCTAssertFalse(t.tick(at: 0.1))
        XCTAssertTrue(t.tick(at: 0.25))
        XCTAssertEqual(t.bloom, 0)
        XCTAssertNil(t.dwellDeadline)
        // Straight out is the first preset; 28° right the second, left the third.
        move(&t, 214, 0, 0.3)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 0))
        move(&t, 214, 28, 0.31)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 1))
        move(&t, 214, -28, 0.32)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 2))
        // Back on the ring the petals stay out for that slot.
        move(&t, 100, 0, 0.4)
        XCTAssertEqual(t.pick, WheelPick(slot: 0))
        XCTAssertEqual(t.bloom, 0)
        // Another slot closes them; one without presets never waits.
        move(&t, 100, 45, 0.5)
        XCTAssertNil(t.bloom)
        XCTAssertNil(t.dwellDeadline)
        // A flick past slot 2 opens it at once.
        t.reset()
        move(&t, 100, 90, 1.0)
        move(&t, 230, 90, 1.02)
        XCTAssertEqual(t.bloom, 2)
        XCTAssertEqual(t.pick, WheelPick(slot: 2, preset: 0))
        move(&t, 230, 146, 1.05)
        XCTAssertEqual(t.pick, WheelPick(slot: 2, preset: 3))
        // Sweeping well past the fan follows the pointer.
        move(&t, 230, 0, 1.1)
        XCTAssertEqual(t.bloom, 0)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 0))
        move(&t, 230, 180, 1.2)
        XCTAssertNil(t.bloom)
        XCTAssertEqual(t.pick, WheelPick(slot: 4))
        // The centre still cancels.
        move(&t, 10, 180, 1.3)
        XCTAssertNil(t.pick)
        XCTAssertNil(t.bloom)
    }

    func testOrbitKeepsTheRingsSlotWhenPushingOut() {
        var t = WheelTracker(spec: WheelSpec(style: .orbit, count: 8), presets: Array(repeating: 2, count: 8))
        move(&t, 100, 0)
        move(&t, 100, 25)
        XCTAssertEqual(t.pick, WheelPick(slot: 0), "sticky on the ring")
        move(&t, 180, 25)
        XCTAssertEqual(t.pick, WheelPick(slot: 0, preset: 1), "and still slot 0 on the outer ring")
        move(&t, 180, 30)
        XCTAssertEqual(t.pick, WheelPick(slot: 1, preset: 0))
    }

    func testSmallWheelsAndOddInput() {
        for style in WheelStyle.allCases {
            for count in [1, 2] {
                var t = WheelTracker(spec: WheelSpec(style: style, count: count), presets: Array(repeating: 3, count: count))
                for a in stride(from: 0.0, to: 360, by: 15) {
                    move(&t, 100, a, a)
                    XCTAssertNotNil(t.pick, "\(style) \(count) at \(a)")
                    move(&t, 230, a, a + 0.5)
                    XCTAssertNotNil(t.pick, "\(style) \(count) out at \(a)")
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
        XCTAssertNil(t.bloom)
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

    func testSelectingCanOpenPetals() {
        var bloom = WheelTracker(spec: WheelSpec(style: .bloom, count: 8), presets: Array(repeating: 3, count: 8))
        bloom.select(WheelPick(slot: 1, preset: 2), open: 1)
        XCTAssertEqual(bloom.pick, WheelPick(slot: 1, preset: 2))
        XCTAssertEqual(bloom.bloom, 1)
        bloom.select(WheelPick(slot: 1), open: 9)
        XCTAssertNil(bloom.bloom, "out of range")
        bloom.select(WheelPick(slot: 1))
        XCTAssertNil(bloom.bloom)
        var dial = WheelTracker(spec: WheelSpec(style: .dial, count: 8), presets: [])
        dial.select(WheelPick(slot: 1), open: 1)
        XCTAssertNil(dial.bloom, "only Bloom has petals")
    }
}
