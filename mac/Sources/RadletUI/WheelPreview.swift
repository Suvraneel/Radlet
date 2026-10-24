import AppKit
import SwiftUI
import RadletKit

/// A small wheel that shows a style and skin, its highlight touring a few
/// slots so the motion shows too. Used by Settings.
@MainActor
package final class WheelPreviewModel: ObservableObject {
    @Published package private(set) var model = WheelModel.preview(style: .dial, count: WheelSpec.defaultCount, skin: Skin())
    private var driver: WheelDriver?
    private var timer: Timer?
    private var step = 0

    package init() {}

    package func update(style: WheelStyle, count: Int, skin: Skin) {
        model = WheelModel.preview(style: style, count: count, skin: skin)
        let driver = WheelDriver(model: model)
        driver.haptics = false
        driver.restart(dwell: 0.24)
        self.driver = driver
        step = 0
        advance()
    }

    package func start() {
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: 1.3, repeats: true) { [weak self] t in
            guard let self else {
                t.invalidate()
                return
            }
            MainActor.assumeIsolated { self.advance() }
        }
    }

    package func stop() {
        timer?.invalidate()
        timer = nil
    }

    private func advance() {
        guard let driver else { return }
        let spec = model.spec
        let tour = WheelTour.picks(spec: spec, presets: (0..<spec.count).map { model.presets(at: $0).count })
        guard !tour.isEmpty else { return }
        let pick = tour[step % tour.count]
        step += 1
        driver.select(pick, open: spec.style == .bloom && pick.preset != nil ? pick.slot : nil, feedback: false)
    }
}
