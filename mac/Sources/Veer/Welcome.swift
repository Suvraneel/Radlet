import AppKit
import SwiftUI

/// The window shown on first launch, and from the menu bar afterwards.
@MainActor
final class WelcomeController {
    private var window: NSWindow?
    private static let shownKey = "welcomeShown"

    /// Shows the guide once per install.
    func showIfFirstLaunch() {
        guard !UserDefaults.standard.bool(forKey: Self.shownKey) else { return }
        UserDefaults.standard.set(true, forKey: Self.shownKey)
        show()
    }

    func show() {
        if window == nil {
            let host = NSHostingController(rootView: WelcomeView(onDone: { [weak self] in self?.window?.close() }))
            let w = NSWindow(contentViewController: host)
            w.title = "Welcome to Veer"
            w.styleMask = [.titled, .closable, .fullSizeContentView]
            w.titlebarAppearsTransparent = true
            w.isReleasedWhenClosed = false
            w.center()
            window = w
        }
        NSApp.activate()
        window?.makeKeyAndOrderFront(nil)
    }
}

private struct WelcomeView: View {
    var onDone: () -> Void
    @ObservedObject private var login = LoginItem.shared

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack(spacing: 12) {
                Image(systemName: "circle.circle")
                    .font(.system(size: 34, weight: .medium))
                    .foregroundStyle(Color.accentColor)
                VStack(alignment: .leading, spacing: 2) {
                    Text("Veer is in your menu bar").font(.title2.weight(.semibold))
                    Text("Convert files without opening anything.").foregroundStyle(.secondary)
                }
            }

            Text("Start dragging files in Finder, then hold a key:")
                .font(.headline)

            VStack(alignment: .leading, spacing: 10) {
                KeyRow(key: "⇧ Shift", title: "Convert", detail: "Turn files into JPEG, PNG, WebP, PDF, MP4, MP3 and more.")
                KeyRow(key: "⌘ Command", title: "Tools", detail: "Crop, resize, compress, remove backgrounds, copy text from pictures, make PDFs searchable.")
                KeyRow(key: "⌃ Control", title: "Recipes", detail: "Your saved multi-step chains, such as Web-ready image.")
            }

            Text("Drop on a slice to run it, or in the middle to cancel. New files appear next to the originals, and nothing is ever overwritten.")
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            Text("Prefer clicking? Right-click files in Finder and choose Services → Convert with Veer, or use the Veer menu.")
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            Divider()

            HStack {
                Toggle("Start Veer when I log in", isOn: $login.enabled)
                Spacer()
                Button("Get Started", action: onDone)
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(28)
        .frame(width: 480)
    }
}

private struct KeyRow: View {
    let key: String
    let title: String
    let detail: String

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text(key)
                .font(.system(size: 12, weight: .semibold, design: .rounded))
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .frame(width: 104, alignment: .center)
                .background(RoundedRectangle(cornerRadius: 6).fill(Color.primary.opacity(0.07)))
                .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.primary.opacity(0.15)))
            VStack(alignment: .leading, spacing: 1) {
                Text(title).font(.system(size: 13, weight: .semibold))
                Text(detail).font(.system(size: 12)).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}
