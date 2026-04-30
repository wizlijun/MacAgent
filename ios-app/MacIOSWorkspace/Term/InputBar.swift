import SwiftUI

struct InputBar: View {
    @Binding var text: String
    let onSendText: (String) -> Void
    let onKey: (InputKey) -> Void
    let onCompose: () -> Void   // M4.4 新增

    private let quickKeys: [(label: String, key: InputKey)] = [
        ("Tab", .tab), ("Esc", .escape),
        ("↑", .arrowUp), ("↓", .arrowDown),
        ("←", .arrowLeft), ("→", .arrowRight),
        ("⌃C", .ctrlC), ("⌃D", .ctrlD),
        ("⌃R", .ctrlR), ("⌃L", .ctrlL),
        ("Home", .home), ("End", .end),
    ]

    var body: some View {
        VStack(spacing: 6) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    ForEach(Array(quickKeys.enumerated()), id: \.offset) { _, item in
                        Button(item.label) { onKey(item.key) }
                            .buttonStyle(.bordered)
                            .controlSize(.small)
                    }
                    // M4.4 新增 ✏️ 按钮
                    Button(action: onCompose) {
                        Image(systemName: "square.and.pencil")
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                }
                .padding(.horizontal, 8)
            }
            HStack(spacing: 8) {
                TextField("type input", text: $text)
                    .textFieldStyle(.roundedBorder)
                    .submitLabel(.send)
                    .onSubmit { send() }
                    .autocapitalization(.none)
                    .disableAutocorrection(true)
                Button("Send") { send() }
                    .buttonStyle(.borderedProminent)
                    .disabled(text.isEmpty)
                Button("⏎") { onKey(.enter) }
                    .buttonStyle(.bordered)
            }
            .padding(.horizontal, 8)
        }
        .padding(.bottom, 8)
        .background(.ultraThinMaterial)
    }

    private func send() {
        guard !text.isEmpty else { return }
        onSendText(text)
        text = ""
    }
}
