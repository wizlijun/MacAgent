import Foundation
import Observation
import WebRTC

@MainActor
@Observable
final class SupervisionStore {
    private(set) var windows: [WindowInfo] = []
    private(set) var entries: [SupervisionEntry] = []
    private(set) var activeTrack: RTCVideoTrack?
    var lastReject: SuperviseRejectInfo?
    var lastInputAck: InputAckRecord?
    weak var glue: RtcGlue?

    init(glue: RtcGlue?) { self.glue = glue }

    func bindIncomingTracks() async {
        guard let glue else { return }
        let stream = glue.incomingVideoTracks()
        for await track in stream {
            activeTrack = track
        }
    }

    func handleWindowsList(_ list: [WindowInfo]) { windows = list }

    func handleSupervisedAck(supId: String, entry: SupervisionEntry) {
        if !entries.contains(where: { $0.supId == supId }) {
            entries.append(entry)
        }
    }

    func handleSuperviseReject(windowId: UInt32, code: String, reason: String) {
        lastReject = SuperviseRejectInfo(windowId: windowId, code: code, reason: reason)
    }

    func handleStreamEnded(supId: String, reason: String) {
        entries.removeAll { $0.supId == supId }
        if entries.isEmpty {
            activeTrack = nil
        }
    }

    func handleSupervisionList(_ list: [SupervisionEntry]) { entries = list }

    func handleGuiInputAck(supId: String, code: String, message: String?) {
        lastInputAck = InputAckRecord(supId: supId, code: code, message: message)
    }

    func refreshWindows() async {
        await glue?.sendCtrl(.listWindows)
    }

    func supervise(windowId: UInt32, viewport: Viewport) async {
        await glue?.sendCtrl(.superviseExisting(windowId: windowId, viewport: viewport))
    }

    func remove(supId: String) async {
        await glue?.sendCtrl(.removeSupervised(supId: supId))
    }

    func clearReject() { lastReject = nil }
}

struct SuperviseRejectInfo: Equatable {
    let windowId: UInt32
    let code: String
    let reason: String
}

struct InputAckRecord: Equatable {
    let supId: String
    let code: String
    let message: String?
}
