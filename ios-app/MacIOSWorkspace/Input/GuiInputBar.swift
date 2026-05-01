import SwiftUI

/// Single-line text + ComposeSheet entry; modifier-aware submit. (GUI input — distinct from Term/InputBar.)
struct GuiInputBar: View {
    let input: InputClient
    @ObservedObject var modState: ModifierState
    @State private var text = ""
    @State private var composeText = ""
    @State private var showCompose = false

    var body: some View {
        HStack(spacing: 8) {
            TextField("输入…", text: $text)
                .textFieldStyle(.roundedBorder)
                .onSubmit { submit() }
            Button {
                composeText = ""
                showCompose = true
            } label: {
                Image(systemName: "pencil")
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .sheet(isPresented: $showCompose) {
            ComposeSheet(
                text: $composeText,
                title: "Compose",
                onSend: { sent in
                    Task { await input.submitText(sent) }
                },
                onCancel: {}
            )
        }
    }

    private func submit() {
        guard !text.isEmpty else { return }
        let mods = modState.active
        let payload = text
        text = ""
        Task {
            if !mods.isEmpty, let firstChar = payload.first.map({ String($0) }) {
                // Modifier active → first char as KeyCombo, rest as KeyText
                await input.keyCombo(mods, firstChar)
                modState.consume()
                if payload.count > 1 {
                    await input.keyText(String(payload.dropFirst()))
                }
            } else {
                await input.submitText(payload)
            }
        }
    }
}
