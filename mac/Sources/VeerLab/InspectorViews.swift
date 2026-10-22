import AppKit
import SwiftUI
import VeerKit
import VeerUI

/// Pick a skin, change any token, and save the result as a skin file.
struct SkinInspector: View {
    @ObservedObject var state: LabState

    private static let groups = ["Colours", "Look", "Type", "Motion"]

    var body: some View {
        VStack(spacing: 0) {
            Form {
                Section("Skin") {
                    Picker("Skin", selection: $state.skinID) {
                        ForEach(state.builtIns) { f in
                            Text(f.skin.name).tag(f.id)
                        }
                        if !state.userFiles.isEmpty {
                            Divider()
                            ForEach(state.userFiles) { f in
                                Text(f.issues.isEmpty ? f.skin.name : "\(f.skin.name) (needs a fix)").tag(f.id)
                            }
                        }
                    }
                    if let file = state.selectedFile, !file.issues.isEmpty {
                        VStack(alignment: .leading, spacing: 2) {
                            ForEach(Array(file.issues.prefix(6).enumerated()), id: \.offset) { _, issue in
                                Text(issue.description)
                            }
                            if file.issues.count > 6 { Text("…and \(file.issues.count - 6) more") }
                        }
                        .font(.caption)
                        .foregroundStyle(.orange)
                        .textSelection(.enabled)
                    }
                    if state.changedOnDisk {
                        VStack(alignment: .leading, spacing: 6) {
                            Label("This file changed on disk.", systemImage: "exclamationmark.triangle")
                                .foregroundStyle(.orange)
                            HStack {
                                Button("Load It") { state.loadFromDisk() }
                                Text("or keep editing and save a copy")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                        }
                    }
                    HStack {
                        Button("Open Skins Folder") { state.openFolder() }
                        Button("Show File") { state.revealSelected() }
                            .disabled(state.selectedFile?.url == nil)
                    }
                    Text(state.folderNote)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                ForEach(Self.groups, id: \.self) { group in
                    Section(group) {
                        ForEach(state.fields.filter { $0.token.group == group }) { field in
                            TokenRow(field: field) { state.reset(field) }
                        }
                    }
                }
            }
            .formStyle(.grouped)
            Divider()
            SkinFilePanel(state: state)
        }
    }
}

/// One token: its name, a control that suits it, and the CSS value.
struct TokenRow: View {
    @ObservedObject var field: TokenField
    let reset: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Text(field.label)
                    .font(.system(size: 12, weight: .medium, design: .monospaced))
                if field.changed {
                    Circle()
                        .fill(Color.accentColor)
                        .frame(width: 6, height: 6)
                        .accessibilityLabel("Changed")
                }
                Spacer(minLength: 4)
                Button(action: reset) {
                    Image(systemName: "arrow.uturn.backward")
                }
                .buttonStyle(.borderless)
                .help("Back to the saved value")
                .opacity(field.changed ? 1 : 0)
                .disabled(!field.changed)
            }
            HStack(spacing: 8) {
                control
            }
            if let problem = field.problem {
                Text(problem)
                    .font(.caption)
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .help(field.token.help)
    }

    @ViewBuilder
    private var control: some View {
        switch field.token.kind {
        case .color:
            ColorPicker("Colour", selection: $field.color, supportsOpacity: true)
                .labelsHidden()
            value(width: nil)
        case .choice(let options):
            if options.count <= 3 {
                Picker("Value", selection: $field.choice) {
                    ForEach(options, id: \.self) { Text($0).tag($0) }
                }
                .labelsHidden()
                .pickerStyle(.segmented)
            } else {
                Picker("Value", selection: $field.choice) {
                    ForEach(options, id: \.self) { Text($0).tag($0) }
                }
                .labelsHidden()
                .pickerStyle(.menu)
            }
        case .number(let range, _, _):
            // No `step:`: on the Mac that draws a tick for every step.
            // TokenField rounds to the step instead.
            Slider(value: $field.number, in: range)
            value(width: 74)
        case .font:
            FontMenu(field: field)
            value(width: nil)
        case .text, .base:
            value(width: nil)
        }
    }

    private func value(width: CGFloat?) -> some View {
        TextField("Value", text: $field.text)
            .labelsHidden()
            .textFieldStyle(.roundedBorder)
            .font(.system(size: 11.5, design: .monospaced))
            .frame(width: width)
    }
}

/// Fonts to choose from: the Mac's own, or any family installed.
struct FontMenu: View {
    @ObservedObject var field: TokenField

    var body: some View {
        Menu {
            Section("The Mac's own") {
                ForEach(SkinFont.Design.allCases.map(\.rawValue), id: \.self) { name in
                    Button(name) { field.text = name }
                }
            }
            Section("Installed") {
                ForEach(NSFontManager.shared.availableFontFamilies.filter { !$0.hasPrefix(".") }, id: \.self) { family in
                    Button(family) { field.text = "\"\(family)\"" }
                }
            }
        } label: {
            Image(systemName: "textformat")
        }
        .menuStyle(.button)
        .buttonStyle(.borderless)
        .menuIndicator(.hidden)
        .fixedSize()
        .help("Choose a font")
    }
}

/// Your changes as a skin file: copy it, or save it to the Skins folder.
struct SkinFilePanel: View {
    @ObservedObject var state: LabState

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("Skin file")
                    .font(.headline)
                Spacer()
                Text(state.changeCount == 0 ? "No changes" : "\(state.changeCount) changed")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            TextField("Name of the new skin", text: $state.newName)
                .textFieldStyle(.roundedBorder)
            ScrollView {
                Text(state.css)
                    .font(.system(size: 11.5, design: .monospaced))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(10)
            }
            .frame(height: 150)
            .background(RoundedRectangle(cornerRadius: 8).fill(Color.primary.opacity(0.05)))
            HStack {
                Button("Revert All") { state.revertAll() }
                    .disabled(state.changeCount == 0)
                Spacer()
                Button("Copy") { state.copyCSS() }
                    .help("Copy the skin file (⇧⌘C)")
                Button("Save as New Skin") { state.saveAsNew() }
                    .buttonStyle(.borderedProminent)
                    .help("Write it to the Skins folder (⌘S)")
            }
            if let status = state.status {
                Text(status)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(14)
    }
}
