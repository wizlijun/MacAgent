import Combine
import SwiftUI

/// Sticky/lock modifier state: tap once = sticky (released after one keypress); tap twice = locked; tap third = clear.
@MainActor
final class ModifierState: ObservableObject {
    @Published var sticky: Set<KeyMod> = []
    @Published var locked: Set<KeyMod> = []

    var active: [KeyMod] { Array(sticky.union(locked)) }

    func tap(_ m: KeyMod) {
        if locked.contains(m) { locked.remove(m); return }
        if sticky.contains(m) { sticky.remove(m); locked.insert(m) }
        else { sticky.insert(m) }
    }

    /// Release sticky modifiers after a keypress; locked stay.
    func consume() { sticky.removeAll() }
}

struct ModifierStickyRow: View {
    @ObservedObject var state: ModifierState

    var body: some View {
        HStack(spacing: 8) {
            modKey("⌘", .cmd)
            modKey("⇧", .shift)
            modKey("⌥", .opt)
            modKey("⌃", .ctrl)
        }
        .padding(.horizontal, 12)
    }

    private func modKey(_ label: String, _ m: KeyMod) -> some View {
        let isLocked = state.locked.contains(m)
        let isSticky = state.sticky.contains(m)
        return Button(label) { state.tap(m) }
            .frame(width: 44, height: 32)
            .background(isLocked ? Color.accentColor : (isSticky ? Color.accentColor.opacity(0.4) : Color.gray.opacity(0.2)))
            .foregroundStyle(isLocked || isSticky ? .white : .primary)
            .clipShape(RoundedRectangle(cornerRadius: 6))
    }
}
