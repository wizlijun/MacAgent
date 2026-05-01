import SwiftUI

/// Esc/Tab/arrows/Enter/Backspace + Cmd-zoom buttons, modifier-aware.
struct SpecialKeyRow: View {
    let input: InputClient
    @ObservedObject var modState: ModifierState

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                key("Esc", "esc")
                key("Tab", "tab")
                key("↑", "up")
                key("↓", "down")
                key("←", "left")
                key("→", "right")
                key("↩", "return")
                key("⌫", "delete")
                Divider().frame(height: 24)
                key("+", "=", mods: [.cmd])
                key("−", "-", mods: [.cmd])
                key("0", "0", mods: [.cmd])
            }
            .padding(.horizontal, 12)
        }
    }

    private func key(_ label: String, _ name: String, mods overrideMods: [KeyMod]? = nil) -> some View {
        Button(label) {
            let mods = overrideMods ?? modState.active
            Task {
                await input.keyCombo(mods, name)
                if overrideMods == nil { modState.consume() }
            }
        }
        .frame(minWidth: 36, minHeight: 32)
        .padding(.horizontal, 8)
        .background(Color.gray.opacity(0.2))
        .clipShape(RoundedRectangle(cornerRadius: 6))
    }
}
