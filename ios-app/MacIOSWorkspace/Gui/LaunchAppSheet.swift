import SwiftUI

/// Hardcoded whitelist picker for `supervise_launch`.
struct LaunchAppSheet: View {
    let store: SupervisionStore
    @Environment(\.dismiss) private var dismiss

    private static let bundles: [(id: String, name: String, icon: String)] = [
        // AI
        ("com.openai.chat",                "ChatGPT",        "bubble.left"),
        ("com.anthropic.claude",           "Claude Desktop", "sparkles"),
        ("com.openai.codex",               "Codex",          "curlybraces"),
        // Browsers
        ("com.google.Chrome",              "Google Chrome",  "globe"),
        ("com.apple.Safari",               "Safari",         "safari"),
        // Editors
        ("com.microsoft.VSCode",           "VS Code",        "chevron.left.forwardslash.chevron.right"),
        ("com.todesktop.230313mzl4w4u92",  "Cursor",         "cursorarrow.click"),
        // Terminals
        ("dev.warp.Warp-Stable",           "Warp",           "terminal.fill"),
        ("com.googlecode.iterm2",          "iTerm",          "terminal"),
        ("com.apple.Terminal",             "Terminal",       "terminal"),
        // Productivity / Design
        ("com.figma.Desktop",              "Figma",          "paintbrush"),
        ("notion.id",                      "Notion",         "doc.text"),
        ("com.linear",                     "Linear",         "list.bullet.rectangle"),
        // Chat
        ("com.tinyspeck.slackmacgap",      "Slack",          "message.fill"),
        ("com.hnc.Discord",                "Discord",        "message.badge.fill"),
        ("ru.keepcoder.Telegram",          "Telegram",       "paperplane"),
        ("com.tencent.xinWeChat",          "微信",            "message"),
        // Media / Office
        ("com.spotify.client",             "Spotify",        "music.note"),
        ("com.microsoft.Word",             "Word",           "doc.fill"),
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
