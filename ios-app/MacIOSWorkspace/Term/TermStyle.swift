import SwiftUI

extension TerminalRun {
    @MainActor
    func attributed() -> AttributedString {
        var s = AttributedString(text)
        if let fg, let color = fg.toSwiftUIColor() {
            s.foregroundColor = color
        }
        if let bg, let color = bg.toSwiftUIColor() {
            s.backgroundColor = color
        }
        var font = Font.system(.body, design: .monospaced)
        if bold { font = font.bold() }
        if italic { font = font.italic() }
        s.font = font
        if underline { s.underlineStyle = .single }
        if inverse {
            let prevFg = s.foregroundColor
            let prevBg = s.backgroundColor
            s.foregroundColor = prevBg ?? .white
            s.backgroundColor = prevFg ?? .black
        }
        return s
    }
}

extension TerminalColor {
    func toSwiftUIColor() -> Color? {
        switch self {
        case .indexed(let value):
            return ansiIndexedColor(value)
        case .rgb(let r, let g, let b):
            return Color(red: Double(r) / 255, green: Double(g) / 255, blue: Double(b) / 255)
        }
    }
}

private func ansiIndexedColor(_ idx: UInt8) -> Color? {
    switch idx {
    case 0: return .black
    case 1: return .red
    case 2: return .green
    case 3: return .yellow
    case 4: return .blue
    case 5: return Color(red: 0.7, green: 0, blue: 0.7)
    case 6: return .cyan
    case 7: return Color(white: 0.85)
    case 8: return Color(white: 0.5)
    case 9: return Color(red: 1, green: 0.4, blue: 0.4)
    case 10: return Color(red: 0.4, green: 1, blue: 0.4)
    case 11: return Color(red: 1, green: 1, blue: 0.4)
    case 12: return Color(red: 0.4, green: 0.4, blue: 1)
    case 13: return Color(red: 1, green: 0.4, blue: 1)
    case 14: return Color(red: 0.4, green: 1, blue: 1)
    case 15: return .white
    default:
        if idx >= 232 {
            let g = Double(idx - 232) / 23.0
            return Color(white: g)
        }
        let n = Int(idx) - 16
        let r = Double((n / 36) % 6) / 5.0
        let g = Double((n / 6) % 6) / 5.0
        let b = Double(n % 6) / 5.0
        return Color(red: r, green: g, blue: b)
    }
}
