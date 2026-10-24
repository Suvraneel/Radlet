import AppKit

/// Which wheel a modifier key opens.
public enum WheelMode: Int, CaseIterable, Sendable {
    case convert
    case tools
    case recipes

    public var title: String {
        switch self {
        case .convert: return "Convert"
        case .tools: return "Tools"
        case .recipes: return "Loadouts"
        }
    }

    /// Shift converts, Command opens tools, Control opens recipes.
    public static func from(_ flags: NSEvent.ModifierFlags) -> WheelMode? {
        if flags.contains(.command) { return .tools }
        if flags.contains(.control) { return .recipes }
        if flags.contains(.shift) { return .convert }
        return nil
    }

    public var next: WheelMode { WheelMode(rawValue: (rawValue + 1) % WheelMode.allCases.count) ?? .convert }
}

/// Bridges AppKit view coordinates (origin bottom-left, y up) and the
/// wheel's offsets from its centre (y down), which `WheelTracker` takes.
public enum WheelGeometry {
    /// The offset of `p`, a point in a square view `side` points across,
    /// from the view's centre.
    public static func offset(appKitPoint p: CGPoint, side: Double) -> (dx: Double, dy: Double) {
        (Double(p.x) - side / 2, side / 2 - Double(p.y))
    }
}
