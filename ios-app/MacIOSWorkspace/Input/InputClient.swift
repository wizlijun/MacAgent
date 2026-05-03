import Combine
import Foundation

@MainActor
final class InputClient: ObservableObject {
    // Test seam — production passes RtcGlue.
    protocol Glue: AnyObject {
        func sendCtrl(_ payload: CtrlPayload) async
    }

    private let supId: String
    private weak var glue: Glue?
    private var lastScrollEmit = Date.distantPast
    private static let scrollThrottle: TimeInterval = 0.016
    private static let pasteThreshold = 32

    init(supId: String, glue: Glue) {
        self.supId = supId
        self.glue = glue
    }

    func tap(normalizedX: CGFloat, normalizedY: CGFloat) async {
        await send(.tap(x: Float(normalizedX), y: Float(normalizedY)))
    }

    func scroll(dx: CGFloat, dy: CGFloat) async {
        let now = Date()
        if now.timeIntervalSince(lastScrollEmit) < Self.scrollThrottle { return }
        lastScrollEmit = now
        await send(.scroll(dx: Float(dx), dy: Float(dy)))
    }

    func keyText(_ s: String) async {
        await send(.keyText(text: s))
    }

    func keyCombo(_ mods: [KeyMod], _ key: String) async {
        await send(.keyCombo(modifiers: mods, key: key))
    }

    // Threshold-based: short → key_text; long → clipboard + Cmd+V.
    func submitText(_ text: String) async {
        if text.count > Self.pasteThreshold {
            await glue?.sendCtrl(.clipboardSet(source: .ios, content: .text(data: text)))
            // Mac dispatches ClipboardSet and KeyCombo on independent tasks; sleep 50ms so pbcopy lands first.
            try? await Task.sleep(nanoseconds: 50_000_000)
            await keyCombo([.cmd], "v")
        } else {
            await keyText(text)
        }
    }

    private func send(_ payload: GuiInput) async {
        await glue?.sendCtrl(.guiInputCmd(supId: supId, payload: payload))
    }
}
