import SwiftUI

/// Off-screen history (plain text) scroll view.
/// Lines accumulated by SessionStore (M3.7); capped at 1000 by caller.
struct HistoryView: View {
    let lines: [String]

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
                    Text(line.isEmpty ? " " : line)
                        .font(.system(.caption, design: .monospaced))
                        .foregroundStyle(.gray)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .padding(8)
        }
        .background(Color.black.opacity(0.95))
    }
}
