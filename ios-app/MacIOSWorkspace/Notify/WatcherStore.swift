import Foundation
import Observation

@MainActor
@Observable
final class WatcherStore {
    /// sid → WatcherInfo 列表
    private(set) var watchers: [String: [WatcherInfo]] = [:]
    /// sid → 最近命中事件（轻量 ring，最近 20 条）
    private(set) var matches: [String: [WatcherMatch]] = [:]
    private weak var glue: RtcGlue?

    init(glue: RtcGlue?) { self.glue = glue }

    func handleList(sid: String, list: [WatcherInfo]) {
        watchers[sid] = list
    }

    func handleMatched(sid: String, watcherId: String, lineText: String) {
        var list = matches[sid] ?? []
        list.insert(WatcherMatch(watcherId: watcherId, lineText: lineText, timestamp: Date()), at: 0)
        if list.count > 20 { list = Array(list.prefix(20)) }
        matches[sid] = list
    }

    // Outbound
    func add(sid: String, regex: String, name: String) async {
        let watcherId = UUID().uuidString
        await glue?.sendCtrl(.watchSession(sid: sid, watcherId: watcherId, regex: regex, name: name))
    }

    func remove(sid: String, watcherId: String) async {
        await glue?.sendCtrl(.unwatchSession(sid: sid, watcherId: watcherId))
    }
}

struct WatcherMatch: Identifiable, Equatable {
    let id = UUID()
    let watcherId: String
    let lineText: String
    let timestamp: Date
}
