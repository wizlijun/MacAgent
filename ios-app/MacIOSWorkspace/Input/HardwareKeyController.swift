import Combine
import GameController
import SwiftUI
import UIKit

final class HardwareKeyController: UIViewController {
    weak var inputClient: InputClient?

    override var canBecomeFirstResponder: Bool { true }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        becomeFirstResponder()
    }

    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        resignFirstResponder()
    }

    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        for press in presses {
            guard let key = press.key else { continue }
            let mods = KeyMapper.modifiers(from: key.modifierFlags)
            if let special = KeyMapper.name(for: key.keyCode) {
                Task { @MainActor in await self.inputClient?.keyCombo(mods, special) }
                return
            }
            let chars = key.characters
            if !chars.isEmpty {
                if !mods.isEmpty {
                    let first = String(chars.first!)
                    Task { @MainActor in await self.inputClient?.keyCombo(mods, first) }
                } else {
                    Task { @MainActor in await self.inputClient?.keyText(chars) }
                }
                return
            }
        }
        super.pressesBegan(presses, with: event)
    }
}

struct HardwareKeyControllerView: UIViewControllerRepresentable {
    let inputClient: InputClient

    func makeUIViewController(context: Context) -> HardwareKeyController {
        let vc = HardwareKeyController()
        vc.inputClient = inputClient
        return vc
    }

    func updateUIViewController(_ uiViewController: HardwareKeyController, context: Context) {
        uiViewController.inputClient = inputClient
    }
}

@MainActor
final class HardwareKeyboardDetector: ObservableObject {
    @Published var isConnected: Bool = GCKeyboard.coalesced != nil

    init() {
        NotificationCenter.default.addObserver(
            forName: .GCKeyboardDidConnect, object: nil, queue: .main
        ) { [weak self] _ in Task { @MainActor [weak self] in self?.isConnected = true } }
        NotificationCenter.default.addObserver(
            forName: .GCKeyboardDidDisconnect, object: nil, queue: .main
        ) { [weak self] _ in Task { @MainActor [weak self] in self?.isConnected = GCKeyboard.coalesced != nil } }
    }
}
