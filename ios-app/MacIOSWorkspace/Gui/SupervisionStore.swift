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
    var lastFitFailed: FitFailedInfo?
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

    // MARK: - M7 actions

    /// Switch the currently active supervised window.
    func requestSwitchActive(supId: String, viewport: Viewport? = nil) {
        guard let glue = self.glue else { return }
        let vp = viewport ?? Viewport(w: 393, h: 760)
        Task { await glue.sendCtrl(.switchActive(supId: supId, viewport: vp)) }
    }

    /// Launch a new app under supervision via bundle id.
    func requestSuperviseLaunch(bundleId: String, viewport: Viewport? = nil) {
        guard let glue = self.glue else { return }
        let vp = viewport ?? Viewport(w: 393, h: 760)
        Task { await glue.sendCtrl(.superviseLaunch(bundleId: bundleId, viewport: vp)) }
    }

    /// Remove a supervised entry by sup_id.
    func requestRemove(supId: String) {
        guard let glue = self.glue else { return }
        Task { await glue.sendCtrl(.removeSupervised(supId: supId)) }
    }

    /// Report current detail-view viewport; only emits if an active sup exists.
    func reportViewport(w: CGFloat, h: CGFloat) {
        guard let glue = self.glue,
              let active = entries.first(where: { $0.status == .active }) else { return }
        let vp = Viewport(w: UInt32(max(1, w)), h: UInt32(max(1, h)))
        Task { await glue.sendCtrl(.viewportChanged(supId: active.supId, viewport: vp)) }
    }

    /// Record latest fit-failed event for UI toast.
    func handleFitFailed(supId: String, reason: String) {
        lastFitFailed = FitFailedInfo(supId: supId, reason: reason, ts: Date())
    }
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

struct FitFailedInfo: Equatable {
    let supId: String
    let reason: String
    let ts: Date
}
