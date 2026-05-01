import SwiftUI

/// Hardcoded whitelist picker for `supervise_launch`.
struct LaunchAppSheet: View {
    let store: SupervisionStore
    @Environment(\.dismiss) private var dismiss

    private static let bundles: [(id: String, name: String, icon: String)] = [
        ("com.openai.chat",      "ChatGPT",        "bubble.left"),
        ("com.anthropic.claude", "Claude Desktop", "sparkles"),
        ("com.google.Chrome",    "Google Chrome",  "globe"),
    ]

    var body: some View {
        NavigationStack {
            List(Self.bundles, id: \.id) { app in
                Button {
                    store.requestSuperviseLaunch(bundleId: app.id)
                    dismiss()
                } label: {
                    Label(app.name, systemImage: app.icon)
                }
            }
            .navigationTitle("启动 App")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消") { dismiss() }
                }
            }
        }
    }
}
