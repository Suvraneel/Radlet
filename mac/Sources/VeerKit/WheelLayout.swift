import Foundation

/// Fits a list of slots onto the wheel. When there are more than fit, the
/// last segment becomes "More…", which opens a menu of the rest.
public enum WheelLayout {
    public static let moreID = "more"

    public static let moreSlot = Slot(id: moreID, label: "More…", symbol: "ellipsis.circle", group: "tool",
                                      action: .object(["kind": .string("more")]))

    public static func fit(_ slots: [Slot], max: Int) -> (visible: [Slot], overflow: [Slot]) {
        guard max > 1, slots.count > max else { return (Array(slots.prefix(Swift.max(max, 0))), []) }
        let shown = Array(slots.prefix(max - 1))
        return (shown + [moreSlot], Array(slots.dropFirst(max - 1)))
    }

    /// Like `fit`, but the slots used most are the ones that stay on the
    /// wheel. They keep the engine's order (most useful first), so a slot
    /// never moves just because another got used more; the rest go behind
    /// "More…", most used first.
    public static func fit(_ slots: [Slot], max: Int, usage: [String: Int]) -> (visible: [Slot], overflow: [Slot]) {
        guard max > 1, slots.count > max else { return fit(slots, max: max) }
        // By position, not id: two recipes can share a name.
        let ranked = slots.indices.sorted { a, b in
            let ca = usage[slots[a].id] ?? 0, cb = usage[slots[b].id] ?? 0
            return ca != cb ? ca > cb : a < b
        }
        let keep = Set(ranked.prefix(max - 1))
        let shown = slots.indices.filter { keep.contains($0) }.map { slots[$0] }
        let rest = ranked.dropFirst(max - 1).map { slots[$0] }
        return (shown + [moreSlot], rest)
    }

    /// Presets for "More…": the first slots behind it, so Orbit and Bloom
    /// reach them without the menu. Their ids are the slots' own.
    public static func morePresets(_ overflow: [Slot]) -> [SlotVariant] {
        overflow.prefix(WheelSpec.maxPetals).map {
            let short = $0.label.hasSuffix("…") ? String($0.label.dropLast()) : $0.label
            return SlotVariant(id: $0.id, label: short, hint: $0.label, action: $0.action)
        }
    }

    /// The app's own Rename tool, offered for any selection.
    public static let renameSlot = Slot(id: "rename", label: "Rename…", symbol: "character.cursor.ibeam",
                                        group: "tool", action: .object(["kind": .string("app_rename")]),
                                        window: "rename")

    /// Adds app-side tools to the engine's list: Rename goes just before
    /// "Copy SHA-256", which stays last.
    public static func withAppTools(_ tools: [Slot]) -> [Slot] {
        guard !tools.contains(where: { $0.id == renameSlot.id }) else { return tools }
        var out = tools
        let at = out.firstIndex { $0.id == "checksum" } ?? out.endIndex
        out.insert(renameSlot, at: at)
        return out
    }
}
