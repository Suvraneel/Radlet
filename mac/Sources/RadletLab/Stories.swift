import RadletKit
import RadletUI

/// One state of the wheel worth looking at, like a story in a storybook.
struct WheelStory: Identifiable {
    enum State: Equatable {
        /// Plays the opening animation each time it's shown.
        case opening
        case resting
        case lastUsed
        case picked(WheelPick)
        /// Orbit: on a slot, close to its presets.
        case near(WheelPick)
        /// Bloom: a slot's petals are out.
        case petals(WheelPick)
        case loading
        case empty
    }

    enum Samples {
        case images
        case tools
    }

    let id: String
    let style: WheelStyle
    let title: String
    /// One line under the stage: what to look for.
    let note: String
    let state: State
    var samples = Samples.images

    var slots: [Slot] {
        switch samples {
        case .images: return WheelSamples.images
        case .tools: return WheelStory.tools
        }
    }

    var filesTitle: String {
        switch samples {
        case .images: return "3 photos"
        case .tools: return "2 files"
        }
    }

    static let all: [WheelStory] = [
        WheelStory(id: "dial.opening", style: .dial, title: "Opening",
                   note: "How the wheel arrives: labels stagger in on the skin's spring.", state: .opening),
        WheelStory(id: "dial.resting", style: .dial, title: "Resting",
                   note: "Open, with the pointer still in the middle.", state: .resting),
        WheelStory(id: "dial.picked", style: .dial, title: "Slot picked",
                   note: "The highlight glides between slots and flashes on each new one.", state: .picked(WheelPick(slot: 2))),
        WheelStory(id: "dial.last", style: .dial, title: "Last used",
                   note: "The dot on the rim marks the slot chosen last time.", state: .lastUsed),
        WheelStory(id: "dial.long", style: .dial, title: "Long labels",
                   note: "Tool names, which wrap and shrink to fit their slot.", state: .picked(WheelPick(slot: 0)),
                   samples: .tools),
        WheelStory(id: "dial.loading", style: .dial, title: "Loading",
                   note: "While the engine works out what the files can become.", state: .loading),
        WheelStory(id: "dial.empty", style: .dial, title: "Nothing to offer",
                   note: "No slots for this selection.", state: .empty),

        WheelStory(id: "orbit.resting", style: .orbit, title: "Resting",
                   note: "Presets wait, dimmed, on the outer ring.", state: .resting),
        WheelStory(id: "orbit.picked", style: .orbit, title: "Slot picked",
                   note: "On the inner ring the slot itself is picked.", state: .picked(WheelPick(slot: 0))),
        WheelStory(id: "orbit.near", style: .orbit, title: "Near the presets",
                   note: "Preset labels brighten as the pointer heads out.", state: .near(WheelPick(slot: 0))),
        WheelStory(id: "orbit.preset", style: .orbit, title: "Preset picked",
                   note: "Out on the second ring, the slot's preset is picked.", state: .picked(WheelPick(slot: 0, preset: 1))),
        WheelStory(id: "orbit.long", style: .orbit, title: "Long labels",
                   note: "Tool names and their presets.", state: .picked(WheelPick(slot: 0, preset: 0)), samples: .tools),

        WheelStory(id: "bloom.resting", style: .bloom, title: "Resting",
                   note: "One ring until you rest on a slot.", state: .resting),
        WheelStory(id: "bloom.picked", style: .bloom, title: "Slot picked",
                   note: "Rest here, or flick outward, to open its presets.", state: .picked(WheelPick(slot: 0))),
        WheelStory(id: "bloom.open", style: .bloom, title: "Petals open",
                   note: "The slot's presets fan out as petals.", state: .petals(WheelPick(slot: 0))),
        WheelStory(id: "bloom.petal", style: .bloom, title: "Petal picked",
                   note: "Letting go here runs the preset.", state: .petals(WheelPick(slot: 0, preset: 1))),
        WheelStory(id: "bloom.long", style: .bloom, title: "Long labels",
                   note: "Tool names and their presets.", state: .petals(WheelPick(slot: 0, preset: 0)), samples: .tools),
    ]

    static func find(_ id: String?) -> WheelStory {
        all.first { $0.id == id } ?? all[0]
    }

    /// Tools with long names, most with presets.
    static let tools: [Slot] = {
        func variant(_ id: String, _ label: String) -> SlotVariant {
            SlotVariant(id: id, label: label, hint: label, action: .null)
        }
        func slot(_ id: String, _ label: String, _ symbol: String, _ presets: [String] = []) -> Slot {
            Slot(id: id, label: label, symbol: symbol, group: "tools", action: .null,
                 variants: presets.enumerated().map { variant("\(id).\($0.offset)", $0.element) })
        }
        return [
            slot("compress", "Compress", "arrow.down.right.and.arrow.up.left", ["WhatsApp", "Discord", "Email", "Strong"]),
            slot("resize", "Resize", "arrow.up.left.and.arrow.down.right", ["50 %", "1920 px", "25 %", "1280 px"]),
            slot("remove_bg", "Remove Background", "person.crop.rectangle", ["And crop"]),
            slot("searchable", "Searchable PDF", "doc.text.magnifyingglass"),
            slot("watermark", "Watermark", "drop", ["Draft", "Copy", "Sample"]),
            slot("privacy", "Privacy Check", "eye.slash", ["Remove all"]),
            slot("rotate", "Rotate Right", "rotate.right", ["180°"]),
            slot("ocr", "Extract Text", "text.viewfinder"),
            slot("split", "Split PDF", "rectangle.split.2x1", ["Every page", "Pairs"]),
            slot("stills", "Video Stills", "film", ["5 stills", "30 stills"]),
            slot("rename", "Rename", "pencil"),
            slot("gif", "Make GIF", "photo.stack"),
        ]
    }()
}
