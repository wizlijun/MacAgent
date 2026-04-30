import Foundation
import Observation
import UIKit

@MainActor
@Observable
final class ClipboardStore {
    private(set) var history: [ClipEntry] = []      // 最近 5 条 Mac → iOS
    private weak var glue: RtcGlue?

    init(glue: RtcGlue?) { self.glue = glue }

    func handleRemote(_ content: ClipContent) {
        switch content {
        case .text(let data):
            let entry = ClipEntry(text: data, timestamp: Date())
            history.insert(entry, at: 0)
            history = Array(history.prefix(5))
            // 自动写到 iOS 系统剪贴板（写入静默，无需权限）
            UIPasteboard.general.string = data
        }
    }

    func sendToMac(_ text: String) async {
        guard !text.isEmpty else { return }
        await glue?.sendCtrl(.clipboardSet(source: .ios, content: .text(data: text)))
    }
}

struct ClipEntry: Identifiable, Equatable {
    let id = UUID()
    let text: String
    let timestamp: Date
}
