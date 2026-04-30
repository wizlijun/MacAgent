import SwiftUI

/// Renders terminal lines passed in from outside (SessionStore wired in M3.7).
struct TermView: View {
    let lines: [TerminalLine]
    let cursorRow: UInt16?
    let cursorCol: UInt16?
    let cursorVisible: Bool
    let cols: UInt16
    let rows: UInt16

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(lines, id: \.index) { line in
                lineView(line)
            }
        }
        .padding(.vertical, 4)
        .padding(.horizontal, 8)
        .background(Color.black.opacity(0.92))
        .foregroundStyle(.white)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @MainActor
    private func lineView(_ line: TerminalLine) -> some View {
        let attributed = line.runs.reduce(AttributedString("")) { acc, run in
            acc + run.attributed()
        }
        return Text(attributed)
            .font(.system(.body, design: .monospaced))
            .lineLimit(1)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}
