import AppKit
import SwiftUI
import RadletKit
import RadletUI

struct LabView: View {
    @ObservedObject var state: LabState

    var body: some View {
        NavigationSplitView {
            StoryList(state: state)
                .navigationSplitViewColumnWidth(min: 180, ideal: 210, max: 280)
        } detail: {
            StageView(state: state)
                .navigationTitle("\(state.story.style.title) · \(state.story.title)")
                .navigationSubtitle(state.changeCount > 0 ? "\(state.edited.name), edited" : state.edited.name)
                .inspector(isPresented: $state.showInspector) {
                    SkinInspector(state: state)
                        .inspectorColumnWidth(min: 330, ideal: 370, max: 520)
                }
                .toolbar { toolbar }
        }
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItemGroup {
            Picker("Layout", selection: $state.layout) {
                Text("One").tag(LabLayout.single)
                Text("All Skins").tag(LabLayout.grid)
            }
            .pickerStyle(.segmented)
            .help("One wheel, or this story in every skin side by side")
            Picker("Slots", selection: $state.count) {
                ForEach(WheelSpec.slotCounts, id: \.self) { n in
                    Text("\(n)").tag(n)
                }
            }
            .pickerStyle(.segmented)
            .help("Slots on the wheel")
        }
        ToolbarItemGroup {
            Menu {
                Picker("Backdrop", selection: $state.backdrop) {
                    ForEach(LabBackdrop.allCases) { b in
                        Text(b.title).tag(b)
                    }
                }
                .pickerStyle(.inline)
                Picker("Appearance", selection: $state.scheme) {
                    Text("Dark").tag(LabScheme.dark)
                    Text("Light").tag(LabScheme.light)
                }
                .pickerStyle(.inline)
                Picker("Zoom", selection: $state.zoom) {
                    ForEach(LabZoom.allCases) { z in
                        Text(z.title).tag(z)
                    }
                }
                .pickerStyle(.inline)
            } label: {
                Label("View", systemImage: "eye")
            }
            .help("Backdrop, light or dark, and zoom")
            Toggle(isOn: $state.reduceMotion) {
                Label("Reduce Motion", systemImage: "accessibility")
            }
            .toggleStyle(.button)
            .help("Draw as with Reduce Motion on: quick fades, no springs")
            Toggle(isOn: $state.slowMotion) {
                Label("Slow Motion", systemImage: "tortoise")
            }
            .toggleStyle(.button)
            .help("Springs four times slower, to study the motion")
            Toggle(isOn: $state.touring) {
                Label("Tour", systemImage: "play")
            }
            .toggleStyle(.button)
            .help("Move the highlight round on its own (⌘T)")
            Button {
                state.replay()
            } label: {
                Label("Replay", systemImage: "arrow.counterclockwise")
            }
            .help("Replay the opening (⌘R)")
        }
        ToolbarItem {
            Button {
                state.showInspector.toggle()
            } label: {
                Label("Skin", systemImage: "sidebar.right")
            }
            .help("Show or hide the skin editor")
        }
    }
}

struct StoryList: View {
    @ObservedObject var state: LabState

    var body: some View {
        List(selection: $state.storyID) {
            ForEach(WheelStyle.allCases) { style in
                Section(style.title) {
                    ForEach(WheelStory.all.filter { $0.style == style }) { story in
                        Text(story.title).tag(story.id)
                    }
                }
            }
        }
        .listStyle(.sidebar)
    }
}

struct StageView: View {
    @ObservedObject var state: LabState

    var body: some View {
        ZStack {
            Backdrop(kind: state.backdrop, scheme: state.scheme)
            if state.layout == .grid {
                SkinGrid(state: state)
            } else if let cell = state.cells.first {
                SingleWheel(cell: cell, zoom: state.zoom, note: state.story.note)
            }
        }
        .environment(\.colorScheme, state.scheme.colorScheme)
        .clipped()
    }
}

/// One big wheel, with what the pointer would run under it.
struct SingleWheel: View {
    @ObservedObject var cell: LabCell
    let zoom: LabZoom
    let note: String

    var body: some View {
        GeometryReader { geo in
            let size = cell.model.spec.size
            let fit = min(Double(geo.size.width) - 64, Double(geo.size.height) - 150) / size
            let scale = zoom == .fit ? min(max(fit, 0.3), 1.6) : (zoom == .actual ? 1 : 2)
            ScrollView([.horizontal, .vertical]) {
                VStack(spacing: 18) {
                    LiveWheel(cell: cell, scale: scale)
                    Caption(cell: cell, model: cell.model, note: note)
                }
                .padding(32)
                .frame(minWidth: geo.size.width, minHeight: geo.size.height)
            }
            .scrollDisabled(zoom == .fit)
        }
    }
}

struct Caption: View {
    @ObservedObject var cell: LabCell
    @ObservedObject var model: WheelModel
    let note: String

    var body: some View {
        VStack(spacing: 4) {
            Text(line)
                .font(.system(size: 13, weight: .medium))
            Text(note)
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
        }
        .multilineTextAlignment(.center)
        .frame(maxWidth: 460)
    }

    private var line: String {
        if let chose = cell.chose { return "Chose \(chose)" }
        if let slot = model.hoveredSlot { return "Letting go runs \(slot.label)" }
        return "Hover to drive the wheel. Click to choose."
    }
}

/// A wheel the pointer can drive, scaled.
struct LiveWheel: View {
    let cell: LabCell
    let scale: Double

    var body: some View {
        let size = cell.model.spec.size
        WheelView(model: cell.model)
            .scaleEffect(scale)
            .frame(width: size * scale, height: size * scale)
            .contentShape(Rectangle())
            .onContinuousHover { phase in cell.hover(phase, scale: scale) }
            .onTapGesture { location in cell.choose(at: location, scale: scale) }
            .accessibilityLabel("\(cell.story.style.title) wheel")
    }
}

/// The story in every skin.
struct SkinGrid: View {
    @ObservedObject var state: LabState

    var body: some View {
        ScrollView {
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 300, maximum: 420), spacing: 28)], spacing: 32) {
                ForEach(state.cells) { cell in
                    let selected = cell.id == state.skinID
                    VStack(spacing: 10) {
                        LiveWheel(cell: cell, scale: 300 / cell.model.spec.size)
                        Button {
                            state.skinID = cell.id
                        } label: {
                            HStack(spacing: 6) {
                                if selected { Image(systemName: "slider.horizontal.3") }
                                Text(state.isEdited(cell.id) ? "\(state.name(of: cell.id)), edited" : state.name(of: cell.id))
                            }
                            .font(.system(size: 12, weight: selected ? .semibold : .regular))
                            .padding(.horizontal, 10)
                            .padding(.vertical, 4)
                            .background(Capsule().fill(selected ? Color.accentColor.opacity(0.22) : Color.primary.opacity(0.07)))
                        }
                        .buttonStyle(.plain)
                        .help(selected ? "The skin you're editing" : "Edit this skin")
                    }
                }
            }
            .padding(32)
        }
    }
}

struct Backdrop: View {
    let kind: LabBackdrop
    let scheme: LabScheme

    var body: some View {
        let dark = scheme == .dark
        switch kind {
        case .wallpaper:
            // Something busy behind the wheel, for glass skins.
            ZStack {
                dark ? Color(red: 0.05, green: 0.06, blue: 0.1) : Color(red: 0.85, green: 0.87, blue: 0.93)
                RadialGradient(colors: [Color(red: 1, green: 0.55, blue: 0.35).opacity(dark ? 0.55 : 0.5), .clear],
                               center: UnitPoint(x: 0.1, y: 0.14), startRadius: 0, endRadius: 480)
                RadialGradient(colors: [Color(red: 0.35, green: 0.55, blue: 1).opacity(0.55), .clear],
                               center: UnitPoint(x: 0.92, y: 0.22), startRadius: 0, endRadius: 500)
                RadialGradient(colors: [Color(red: 0.67, green: 0.35, blue: 1).opacity(0.45), .clear],
                               center: UnitPoint(x: 0.5, y: 1.04), startRadius: 0, endRadius: 560)
            }
        case .plain:
            dark ? Color(white: 0.11) : Color(white: 0.94)
        case .checker:
            Checkerboard(dark: dark)
        }
    }
}

/// Squares, to show what's see-through.
struct Checkerboard: View {
    let dark: Bool

    var body: some View {
        Canvas { ctx, size in
            let side = 12.0
            ctx.fill(Path(CGRect(origin: .zero, size: size)), with: .color(Color(white: dark ? 0.16 : 0.97)))
            var squares = Path()
            var y = 0.0
            var row = 0
            while y < Double(size.height) {
                var x = row % 2 == 0 ? 0.0 : side
                while x < Double(size.width) {
                    squares.addRect(CGRect(x: x, y: y, width: side, height: side))
                    x += side * 2
                }
                y += side
                row += 1
            }
            ctx.fill(squares, with: .color(Color(white: dark ? 0.22 : 0.88)))
        }
    }
}
