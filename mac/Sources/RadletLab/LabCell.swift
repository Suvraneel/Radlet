import AppKit
import SwiftUI
import RadletKit
import RadletUI

/// One wheel on the stage: a story drawn in one skin. Hover drives it with
/// the app's own tracker; leaving puts the story back.
@MainActor
final class LabCell: ObservableObject, Identifiable {
    let id: String
    let story: WheelStory
    let model: WheelModel
    let driver: WheelDriver
    /// What a click ran, shown for a moment.
    @Published private(set) var chose: String?
    /// The pointer is on this wheel.
    private(set) var live = false
    private var tourStep = 0
    /// Bumped by each replay or click, so a late step can't undo a newer one.
    private var generation = 0

    init(id: String, story: WheelStory, count: Int, skin: Skin, reduceMotion: Bool) {
        self.id = id
        self.story = story
        let model = WheelModel()
        self.model = model
        driver = WheelDriver(model: model)
        switch story.state {
        case .loading, .empty: model.full = .empty
        default: model.full = Wheel(convert: story.slots, tools: [], recipes: [])
        }
        model.loading = story.state == .loading
        model.title = story.filesTitle
        model.configure(style: story.style, count: count, skin: skin, reduceMotion: reduceMotion)
        model.apply(.convert, usage: [:])
        if story.samples == .tools { model.subtitle = "Tools" }
        model.lastID = story.state == .lastUsed && model.positions.count > 3 ? model.positions[3].id : nil
        driver.restart(dwell: 0.24)
        model.shown = true
        restore()
        if story.state == .opening { replay() }
    }

    /// Restyles without losing the pick.
    func restyle(skin: Skin, reduceMotion: Bool) {
        model.configure(style: story.style, count: model.slotCount, skin: skin, reduceMotion: reduceMotion)
    }

    /// Puts the story's own state back, gliding from wherever it is.
    func restore() {
        driver.cancelDwell()
        model.proximity = 0
        switch story.state {
        case .opening, .resting, .lastUsed, .loading, .empty:
            driver.select(nil, feedback: false)
        case .picked(let p):
            driver.select(valid(p), feedback: false)
        case .near(let p):
            driver.select(valid(p), feedback: false)
            model.proximity = 0.6
        case .petals(let p):
            let pick = valid(p)
            driver.select(pick, open: pick?.slot, feedback: false)
        }
    }

    /// A story's pick, trimmed to what this wheel has.
    private func valid(_ p: WheelPick) -> WheelPick? {
        guard p.slot < model.positions.count else { return nil }
        guard let v = p.preset else { return p }
        return v < model.shownPresets(at: p.slot).count ? p : WheelPick(slot: p.slot)
    }

    /// The pointer, in the wheel's own points from its top-left corner.
    func hover(_ phase: HoverPhase, scale: Double) {
        let size = model.spec.size
        switch phase {
        case .active(let location):
            live = true
            // Between a click and the wheel reopening.
            guard chose == nil else { return }
            let dx = (Double(location.x) - size * scale / 2) / scale
            let dy = (Double(location.y) - size * scale / 2) / scale
            if (dx * dx + dy * dy).squareRoot() > driver.reach {
                if driver.mouseOwnsPick { driver.pointerLeft() }
            } else {
                driver.pointer(dx: dx, dy: dy)
            }
        case .ended:
            live = false
            restore()
        }
    }

    /// A click at `location` (as in `hover`): picks what's under it and runs
    /// it the way the app does, then reopens. Beyond the wheel it does
    /// nothing, where the app would cancel.
    func choose(at location: CGPoint, scale: Double) {
        guard chose == nil else { return }
        let size = model.spec.size
        let dx = (Double(location.x) - size * scale / 2) / scale
        let dy = (Double(location.y) - size * scale / 2) / scale
        guard (dx * dx + dy * dy).squareRoot() <= driver.reach else { return }
        driver.pointer(dx: dx, dy: dy)
        guard let slot = model.hoveredSlot else { return }
        chose = slot.label
        generation += 1
        let mine = generation
        withAnimation(.timingCurve(0.2, 0.8, 0.2, 1, duration: 0.2)) {
            model.committed = true
            model.shown = false
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.generation == mine else { return }
                self.chose = nil
                self.driver.clear()
                self.restore()
                self.replay()
            }
        }
    }

    /// Closes the wheel at once and opens it again, animated.
    func replay() {
        generation += 1
        let mine = generation
        // A click waiting to reopen is overtaken by this.
        chose = nil
        var still = Transaction()
        still.disablesAnimations = true
        withTransaction(still) {
            model.shown = false
            model.committed = false
        }
        // Next turn of the run loop, so the closed state draws first.
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.0 / 60) { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.generation == mine else { return }
                withAnimation(self.model.look.spring) { self.model.shown = true }
            }
        }
    }

    /// The next stop on a tour of the wheel's motion, unless the pointer
    /// is driving it.
    func advanceTour() {
        guard !live, model.shown else { return }
        let spec = model.spec
        let tour = WheelTour.picks(spec: spec, presets: (0..<spec.count).map { model.presets(at: $0).count })
        guard !tour.isEmpty else { return }
        let pick = tour[tourStep % tour.count]
        tourStep += 1
        // The flash, but no trackpad tap every second.
        let haptics = driver.haptics
        driver.haptics = false
        driver.select(pick, open: spec.style == .bloom && pick.preset != nil ? pick.slot : nil)
        driver.haptics = haptics
    }

    func endTour() {
        tourStep = 0
        if !live { restore() }
    }
}
