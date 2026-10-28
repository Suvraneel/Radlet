import AppKit
import SwiftUI
import RadletKit

/// Turns pointer moves into what the wheel shows: runs a WheelTracker and
/// copies its pick into the model, gliding or snapping the highlights, with
/// a flash and a haptic tap on each new pick. The app's wheel and the Wheel
/// Lab both drive their wheels with this, so they behave the same.
@MainActor
package final class WheelDriver {
    package let model: WheelModel
    /// A light tap on a Force Touch trackpad each time the pick changes.
    package var haptics = true
    /// The current pick came from the mouse, not the arrow keys.
    package private(set) var mouseOwnsPick = false

    private var tracker = WheelTracker(spec: WheelSpec(style: .dial, count: 0), presets: [])
    private var dwellWork: DispatchWorkItem?

    package init(model: WheelModel) {
        self.model = model
    }

    private static var now: Double { ProcessInfo.processInfo.systemUptime }

    /// Starts tracking afresh for what the model shows now. Call after
    /// every `model.apply`.
    package func restart(dwell: Double) {
        tracker = WheelTracker(spec: model.spec,
                               presets: (0..<model.spec.count).map { model.presets(at: $0).count })
        tracker.dwell = dwell
        dwellWork?.cancel()
        mouseOwnsPick = false
    }

    /// The pointer is at (`dx`, `dy`) from the centre, y down.
    package func pointer(dx: Double, dy: Double) {
        let changed = tracker.move(dx: dx, dy: dy, at: WheelDriver.now)
        // Open presets count too: after an overshoot nothing is picked, but
        // leaving the wheel should still fold them away.
        if changed { mouseOwnsPick = tracker.pick != nil || tracker.focus != nil }
        if model.spec.style == .orbit {
            let p = min(max((tracker.distance - (model.spec.presetInner - 56)) / 56, 0), 1)
            let stepped = (p * 10).rounded() / 10
            if stepped != model.proximity { model.proximity = stepped }
        }
        if changed { sync(feedback: true) }
        scheduleDwell()
    }

    /// The pointer left the wheel. With `keepOpen` (still dragging, the key
    /// still held) open presets stay out; nothing is picked.
    package func pointerLeft(keepOpen: Bool = false) {
        guard tracker.pick != nil || model.pick != nil || tracker.focus != nil || model.focus != nil
                || model.proximity != 0 else { return }
        tracker.leave(keepOpen: keepOpen)
        dwellWork?.cancel()
        model.proximity = 0
        mouseOwnsPick = tracker.focus != nil
        sync(feedback: false)
    }

    /// How far out the mouse still counts when it isn't dragging: past the
    /// rim, or past Bloom's petals.
    package var reach: Double {
        let s = model.spec
        return s.style == .bloom ? s.petalRadius + 64 : s.rim + 24
    }

    /// Picks a slot or preset directly: the arrow keys, or a preview.
    /// `open` puts that slot's presets all out (Orbit and Bloom). Without `feedback` there's
    /// no flash or tap.
    package func select(_ pick: WheelPick?, open: Int? = nil, feedback: Bool = true) {
        mouseOwnsPick = false
        tracker.select(pick, open: open)
        sync(feedback: feedback)
    }

    package func cancelDwell() {
        dwellWork?.cancel()
    }

    /// Lets go of everything at once, without animating: the wheel has gone.
    package func clear() {
        dwellWork?.cancel()
        tracker.reset()
        model.pick = nil
        model.focus = nil
    }

    /// Copies the tracker's state into the model. A highlight glides from
    /// slot to slot, but snaps into place when it first appears; the view
    /// reads `ringGlides` and `presetGlides` to tell which.
    private func sync(feedback: Bool) {
        let old = model.pick
        let new = tracker.pick
        let spec = model.spec
        var ring = model.ringAngle
        var outer = model.presetAngle
        var width = model.presetWidth
        var outerGlides = false
        if let p = new {
            let target = spec.angle(of: p.slot)
            ring = old == nil ? target : WheelAngle.turn(from: model.ringAngle, to: target)
            if let v = p.preset, spec.style == .orbit {
                let n = model.shownPresets(at: p.slot).count
                let t = spec.presetAngle(slot: p.slot, preset: v, shown: n)
                // Faceted half cells change slope, so they snap instead.
                outerGlides = old?.preset != nil && model.look.sides(spec) == 0
                outer = outerGlides ? WheelAngle.turn(from: model.presetAngle, to: t) : t
                width = n <= 1 ? spec.span : spec.span / 2
            }
        }
        withAnimation(model.look.spring) {
            model.ringGlides = old != nil
            model.presetGlides = outerGlides
            model.pick = new
            model.focus = tracker.focus
            if let f = tracker.focus { model.lastFocus = f }
            model.ringAngle = ring
            model.presetAngle = outer
            model.presetWidth = width
        }
        guard feedback, new != nil else { return }
        if haptics {
            NSHapticFeedbackManager.defaultPerformer.perform(.alignment, performanceTime: .now)
        }
        guard !model.look.reduceMotion else { return }
        // The flash layers animate on their own value (see RingHighlight).
        model.flash = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { [weak self] in
            MainActor.assumeIsolated { self?.model.flash = false }
        }
    }

    /// Opens Bloom's presets, or folds away open ones, once the pointer has
    /// rested long enough, even if it doesn't move again.
    private func scheduleDwell() {
        dwellWork?.cancel()
        guard let deadline = tracker.dwellDeadline else { return }
        let work = DispatchWorkItem { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.tracker.tick(at: WheelDriver.now) else { return }
                // A tap when presets fan out; folding away is quiet.
                self.sync(feedback: self.tracker.focus != nil)
            }
        }
        dwellWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + max(0, deadline - WheelDriver.now), execute: work)
    }
}
