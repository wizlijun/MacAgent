import Foundation
import WebRTC

actor RtcClient {
    private let factory: RTCPeerConnectionFactory
    private let pc: RTCPeerConnection
    private var ctrlChannel: RTCDataChannel?
    private var channelDelegate: ChannelDelegate?
    private var observer: PeerObserver?

    enum PeerState { case new, connecting, connected, disconnected, failed, closed }

    private var candidateContinuation: AsyncStream<String>.Continuation?
    private var stateContinuation: AsyncStream<PeerState>.Continuation?
    private var ctrlContinuation: AsyncStream<String>.Continuation?

    nonisolated func candidates() -> AsyncStream<String> {
        AsyncStream { continuation in
            Task { await self.setCandidateContinuation(continuation) }
        }
    }
    nonisolated func peerStates() -> AsyncStream<PeerState> {
        AsyncStream { continuation in
            Task { await self.setStateContinuation(continuation) }
        }
    }
    nonisolated func ctrlMessages() -> AsyncStream<String> {
        AsyncStream { continuation in
            Task { await self.setCtrlContinuation(continuation) }
        }
    }

    private func setCandidateContinuation(_ c: AsyncStream<String>.Continuation) {
        candidateContinuation = c
    }
    private func setStateContinuation(_ c: AsyncStream<PeerState>.Continuation) {
        stateContinuation = c
    }
    private func setCtrlContinuation(_ c: AsyncStream<String>.Continuation) {
        ctrlContinuation = c
    }

    init(iceServers: [[String: Any]]) {
        RTCInitializeSSL()
        self.factory = RTCPeerConnectionFactory()
        let cfg = RTCConfiguration()
        cfg.iceServers = iceServers.map { dict in
            let urls: [String]
            if let arr = dict["urls"] as? [String] {
                urls = arr
            } else if let str = dict["urls"] as? String {
                urls = [str]
            } else {
                urls = []
            }
            return RTCIceServer(
                urlStrings: urls,
                username: dict["username"] as? String,
                credential: dict["credential"] as? String
            )
        }
        cfg.sdpSemantics = .unifiedPlan
        cfg.iceTransportPolicy = .all
        let constraints = RTCMediaConstraints(mandatoryConstraints: nil, optionalConstraints: nil)

        let observer = PeerObserver()
        self.observer = observer
        self.pc = factory.peerConnection(with: cfg, constraints: constraints, delegate: observer)!

        observer.onCandidate = { [weak self] c in
            Task { await self?.emitCandidate(c) }
        }
        observer.onState = { [weak self] s in
            Task { await self?.emitState(s) }
        }
        observer.onDataChannel = { [weak self] dc in
            Task { await self?.attachIncomingChannel(dc) }
        }
    }

    func openCtrlChannel() -> Bool {
        let cfg = RTCDataChannelConfiguration()
        cfg.isOrdered = true
        guard let dc = pc.dataChannel(forLabel: "ctrl", configuration: cfg) else { return false }
        attachChannelDelegate(dc)
        ctrlChannel = dc
        return true
    }

    private func attachIncomingChannel(_ dc: RTCDataChannel) {
        attachChannelDelegate(dc)
        ctrlChannel = dc
    }

    private func attachChannelDelegate(_ dc: RTCDataChannel) {
        let dele = ChannelDelegate()
        dele.onMessage = { [weak self] msg in
            Task { await self?.ctrlContinuation?.yield(msg) }
        }
        dc.delegate = dele
        channelDelegate = dele  // keep alive
    }

    private func emitCandidate(_ c: RTCIceCandidate) {
        let dict: [String: Any] = [
            "candidate": c.sdp,
            "sdpMid": c.sdpMid as Any,
            "sdpMLineIndex": c.sdpMLineIndex,
        ]
        if let json = try? JSONSerialization.data(withJSONObject: dict),
           let s = String(data: json, encoding: .utf8) {
            candidateContinuation?.yield(s)
        }
    }

    private func emitState(_ s: RTCIceConnectionState) {
        let mapped: PeerState
        switch s {
        case .new: mapped = .new
        case .checking: mapped = .connecting
        case .connected, .completed: mapped = .connected
        case .disconnected: mapped = .disconnected
        case .failed: mapped = .failed
        case .closed: mapped = .closed
        default: mapped = .new
        }
        stateContinuation?.yield(mapped)
    }

    func createOffer() async throws -> String {
        let constraints = RTCMediaConstraints(mandatoryConstraints: nil, optionalConstraints: nil)
        let offer = try await pc.offer(for: constraints)
        try await pc.setLocalDescription(offer)
        return offer.sdp
    }

    func createAnswer() async throws -> String {
        let constraints = RTCMediaConstraints(mandatoryConstraints: nil, optionalConstraints: nil)
        let answer = try await pc.answer(for: constraints)
        try await pc.setLocalDescription(answer)
        return answer.sdp
    }

    func applyRemoteOffer(_ sdp: String) async throws {
        let desc = RTCSessionDescription(type: .offer, sdp: sdp)
        try await pc.setRemoteDescription(desc)
    }

    func applyRemoteAnswer(_ sdp: String) async throws {
        let desc = RTCSessionDescription(type: .answer, sdp: sdp)
        try await pc.setRemoteDescription(desc)
    }

    func applyRemoteCandidate(_ candidateJson: String) async throws {
        guard let data = candidateJson.data(using: .utf8),
              let dict = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let sdp = dict["candidate"] as? String else {
            return
        }
        let candidate = RTCIceCandidate(
            sdp: sdp,
            sdpMLineIndex: (dict["sdpMLineIndex"] as? Int32) ?? 0,
            sdpMid: dict["sdpMid"] as? String
        )
        try await pc.add(candidate)
    }

    func sendCtrl(_ json: String) {
        guard let dc = ctrlChannel, let data = json.data(using: .utf8) else { return }
        dc.sendData(RTCDataBuffer(data: data, isBinary: false))
    }

    func close() {
        pc.close()
        candidateContinuation?.finish()
        stateContinuation?.finish()
        ctrlContinuation?.finish()
        RTCCleanupSSL()
    }
}

final class PeerObserver: NSObject, RTCPeerConnectionDelegate, @unchecked Sendable {
    nonisolated(unsafe) var onCandidate: ((RTCIceCandidate) -> Void)?
    nonisolated(unsafe) var onState: ((RTCIceConnectionState) -> Void)?
    nonisolated(unsafe) var onDataChannel: ((RTCDataChannel) -> Void)?

    func peerConnection(_ peerConnection: RTCPeerConnection, didChange stateChanged: RTCSignalingState) {}
    func peerConnection(_ peerConnection: RTCPeerConnection, didAdd stream: RTCMediaStream) {}
    func peerConnection(_ peerConnection: RTCPeerConnection, didRemove stream: RTCMediaStream) {}
    func peerConnectionShouldNegotiate(_ peerConnection: RTCPeerConnection) {}
    func peerConnection(_ peerConnection: RTCPeerConnection, didChange newState: RTCIceConnectionState) {
        onState?(newState)
    }
    func peerConnection(_ peerConnection: RTCPeerConnection, didChange newState: RTCIceGatheringState) {}
    func peerConnection(_ peerConnection: RTCPeerConnection, didGenerate candidate: RTCIceCandidate) {
        onCandidate?(candidate)
    }
    func peerConnection(_ peerConnection: RTCPeerConnection, didRemove candidates: [RTCIceCandidate]) {}
    func peerConnection(_ peerConnection: RTCPeerConnection, didOpen dataChannel: RTCDataChannel) {
        onDataChannel?(dataChannel)
    }
}

final class ChannelDelegate: NSObject, RTCDataChannelDelegate, @unchecked Sendable {
    nonisolated(unsafe) var onMessage: ((String) -> Void)?

    func dataChannel(_ dataChannel: RTCDataChannel, didReceiveMessageWith buffer: RTCDataBuffer) {
        if let s = String(data: buffer.data, encoding: .utf8) {
            onMessage?(s)
        }
    }
    func dataChannelDidChangeState(_ dataChannel: RTCDataChannel) {}
}
