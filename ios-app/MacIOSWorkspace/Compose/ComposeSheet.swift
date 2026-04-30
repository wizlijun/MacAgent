import SwiftUI

struct ComposeSheet: View {
    @Binding var text: String
    let title: String
    let onSend: (String) -> Void
    let onCancel: () -> Void

    @FocusState private var editorFocused: Bool
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                TextEditor(text: $text)
                    .font(.system(.body, design: .monospaced))
                    .focused($editorFocused)
                    .padding(8)
                    .background(Color(uiColor: .systemBackground))
            }
            .navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Cancel") {
                        onCancel()
                        dismiss()
                    }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Send") {
                        // 强制 commit IME 组字（中文拼音 / 五笔等输入法在按 Send 前可能还在组字状态）
                        UIApplication.shared.sendAction(
                            #selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil
                        )
                        let toSend = text
                        onSend(toSend)
                        text = ""
                        dismiss()
                    }
                    .disabled(text.isEmpty)
                }
            }
        }
        .onAppear { editorFocused = true }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
    }
}

#Preview {
    @Previewable @State var text = ""
    return ComposeSheet(
        text: $text,
        title: "Compose · preview",
        onSend: { sent in print("send:", sent) },
        onCancel: { print("cancel") }
    )
}
