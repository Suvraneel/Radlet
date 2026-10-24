import AppKit
import SwiftUI
import RadletKit
import RadletUI

// The Wheel Lab: a storybook for the wheel. Every style and state, drawn
// by the app's own code (RadletUI), with a skin editor that writes skin
// files. Run it with `swift run --package-path mac RadletLab`; add
// `--skins <folder>` to work on skins somewhere other than the Skins folder.

@main
struct WheelLabApp: App {
    @NSApplicationDelegateAdaptor(LabDelegate.self) private var delegate
    @StateObject private var state = LabState(skinsDir: LabArguments.skinsDir)

    var body: some Scene {
        Window("Radlet Wheel Lab", id: "lab") {
            LabView(state: state)
                .frame(minWidth: 1000, minHeight: 660)
        }
        .defaultSize(width: 1360, height: 880)
        .commands { LabCommands(state: state) }
    }
}

final class LabDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        // Started from Terminal, so it has to ask for a Dock icon and the
        // front of the screen.
        MainActor.assumeIsolated {
            NSApp.setActivationPolicy(.regular)
            NSApp.activate()
        }
        // macOS may decline to activate an app Terminal started; at least
        // put the window in front.
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                for window in NSApp.windows where window.isVisible && window.canBecomeMain {
                    window.orderFrontRegardless()
                }
            }
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

enum LabArguments {
    /// `--skins <folder>`, or the app's own Skins folder.
    static var skinsDir: URL {
        let args = CommandLine.arguments
        if let i = args.firstIndex(of: "--skins"), i + 1 < args.count {
            let path = (args[i + 1] as NSString).expandingTildeInPath
            return URL(fileURLWithPath: path, isDirectory: true).standardizedFileURL
        }
        return SkinLibrary.defaultDirectory
    }
}

struct LabCommands: Commands {
    let state: LabState

    var body: some Commands {
        CommandMenu("Wheel") {
            Button("Replay Opening") { state.replay() }
                .keyboardShortcut("r")
            Button("Start or Stop Tour") { state.touring.toggle() }
                .keyboardShortcut("t")
            Divider()
            Button("Copy Skin File") { state.copyCSS() }
                .keyboardShortcut("c", modifiers: [.command, .shift])
            Button("Save as New Skin") { state.saveAsNew() }
                .keyboardShortcut("s")
            Button("Reload Skins Folder") { state.reloadFiles(force: true) }
                .keyboardShortcut("r", modifiers: [.command, .shift])
        }
    }
}
