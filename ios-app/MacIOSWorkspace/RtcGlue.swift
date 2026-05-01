import Foundation
import WebRTC

enum GlueState: Equatable { case idle, fetchingTurn, signalingConnected, negotiating, connected, reconnecting, failed }

actor RtcGlue {
    private let pair: PairStore.PairedPair
    private var rtc: RtcClient?
    private var ws: SignalingClient?
    private var stateContinuation: AsyncStream<GlueState>.Continuation?
    private var ctrlPayloadContinuation: AsyncStream<CtrlPayload>.Continuation?
    private var incomingVideoContinuation: AsyncStream<RTCVideoTrack>.Continuation?

    // Heartbeat state (iOS is answerer; Mac sends hb, iOS sends hb_ack)
    private var hbAckCount: Int = 0
    private var lastAckDate: Date? = nil
    private var sharedSecret: Data? = nil

    init(pair: PairStore.PairedPair) {
        self.pair = pair
    }

    nonisolated func states() -> AsyncStream<GlueState> {
        AsyncStream { continuation in
            Task { await self.setContinuation(continuation) }
        }
    }

    private func setContinuation(_ c: AsyncStream<GlueState>.Continuation) {
        stateContinuation = c
    }

    nonisolated func ctrlMessages() -> AsyncStream<CtrlPayload> {
        AsyncStream { continuation in
            Task { await self.setCtrlPayloadContinuation(continuation) }
        }
    }

    private func setCtrlPayloadContinuation(_ c: AsyncStream<CtrlPayload>.Continuation) {
        ctrlPayloadContinuation = c
    }

    nonisolated func incomingVideoTracks() -> AsyncStream<RTCVideoTrack> {
        // Stash the continuation up-front so callers that bind before run()
        // completes still receive forwarded tracks once rtc is wired in run().
        AsyncStream { continuation in
            Task { await self.setIncomingVideoContinuation(continuation) }
        }
    }

    private func setIncomingVideoContinuation(_ c: AsyncStream<RTCVideoTrack>.Continuation) {
        incomingVideoContinuation = c
    }

    private func forwardVideo(_ track: RTCVideoTrack) {
        incomingVideoContinuation?.yield(track)
    }

    func sendCtrl(_ payload: CtrlPayload) async {
        guard let ss = sharedSecret,
              let signed = try? SignedCtrl.sign(payload, sharedSecret: ss),
              let json = try? JSONEncoder().encode(signed),
              let str = String(data: json, encoding: .utf8) else { return }
        await rtc?.sendCtrl(str)
    }

    private func emit(_ s: GlueState) {
        stateContinuation?.yield(s)
    }

    func heartbeatStats() -> (sent: Int, acked: Int, missed: Int, lastAck: Date?) {
        (0, hbAckCount, 0, lastAckDate)
    }

    func run() async {
        emit(.fetchingTurn)
        guard let ice = await fetchTurnCred() else { emit(.failed); return }

        guard let secret = Data(base64Encoded: pair.deviceSecretB64) else { emit(.failed); return }
        guard let ws = try? SignalingClient(workerURL: pair.workerURL, pairID: pair.pairId, deviceSecret: secret) else {
            emit(.failed); return
        }
        self.ws = ws
        emit(.signalingConnected)

        // Derive shared_secret for ctrl HMAC
        if let priv = (try? Keychain.get("ios.local.privkey")) ?? nil,
           let keys = try? PairKeys.from(privateKeyData: priv),
           let ss = try? keys.deriveSharedSecret(peerPubB64: pair.peerPubB64) {
            self.sharedSecret = ss
        }

        let rtc = RtcClient(iceServers: ice)
        self.rtc = rtc
        _ = await rtc.openCtrlChannel()

        emit(.negotiating)

        // Forward ICE candidates to signaling
        Task {
            for await candidateJson in rtc.candidates() {
                await self.sendIce(candidateJson)
            }
        }

        // Forward peer state changes to glue state
        Task {
            for await state in rtc.peerStates() {
                await self.handleState(state)
            }
        }

        // ctrl channel recv loop — handle hb/hb_ack
        Task {
            for await msg in rtc.ctrlMessages() {
                await self.handleCtrlMessage(msg)
            }
        }

        // Forward incoming video tracks to any continuation registered via
        // incomingVideoTracks(); registered continuation may pre-date run().
        Task {
            for await track in rtc.incomingVideoTracks() {
                await self.forwardVideo(track)
            }
        }

        // Create and send offer
        do {
            let offer = try await rtc.createOffer()
            let frame = try JSONSerialization.data(withJSONObject: ["kind": "sdp", "side": "offer", "sdp": offer])
            try await ws.send(String(data: frame, encoding: .utf8)!)
        } catch {
            emit(.failed); return
        }

        // Signaling recv loop
        while let text = try? await ws.recv() {
            guard let data = text.data(using: .utf8),
                  let dict = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let kind = dict["kind"] as? String else { continue }
            switch kind {
            case "sdp":
                guard let side = dict["side"] as? String, let sdp = dict["sdp"] as? String else { break }
                if side == "answer" {
                    try? await rtc.applyRemoteAnswer(sdp)
                } else if side == "offer" {
                    try? await rtc.applyRemoteOffer(sdp)
                    if let answer = try? await rtc.createAnswer() {
                        let payload: [String: Any] = ["kind": "sdp", "side": "answer", "sdp": answer]
                        if let f = try? JSONSerialization.data(withJSONObject: payload),
                           let s = String(data: f, encoding: .utf8) {
                            try? await ws.send(s)
                        }
                    }
                }
            case "ice":
                if let candidate = dict["candidate"] as? String {
                    try? await rtc.applyRemoteCandidate(candidate)
                }
            case "restart":
                // Mac 主动触发 ICE restart，等待对端发来新 offer 即可（已由 sdp/offer 分支处理）。
                // 这里只做 logging 与状态汇报。
                print("rtc_glue: received restart hint from peer")
            default:
                break
            }
        }
        incomingVideoContinuation?.finish()
    }

    private func handleState(_ s: RtcClient.PeerState) {
        switch s {
        case .connected: emit(.connected)
        // ICE flap: peer briefly disconnected; ICE-restart path will recover.
        case .disconnected: emit(.reconnecting)
        case .failed, .closed: emit(.failed)
        default: emit(.negotiating)
        }
    }

    private func handleCtrlMessage(_ msg: String) {
        guard let ss = sharedSecret else { return }
        guard let data = msg.data(using: .utf8),
              let signed = try? JSONDecoder().decode(SignedCtrl.self, from: data),
              (try? signed.verify(sharedSecret: ss)) != nil else { return }

        switch signed.payload {
        case .heartbeat(_, let nonce):
            // Reply with HeartbeatAck (same nonce, new ts)
            let replyNonce = nonce
            let ts = UInt64(Date().timeIntervalSince1970 * 1000)
            guard let ack = try? SignedCtrl.sign(.heartbeatAck(ts: ts, nonce: replyNonce), sharedSecret: ss),
                  let json = try? JSONEncoder().encode(ack),
                  let str = String(data: json, encoding: .utf8) else { return }
            Task { await self.rtc?.sendCtrl(str) }
            hbAckCount += 1
            lastAckDate = Date()

        case .heartbeatAck:
            // iOS is answerer; Mac sends hb. If Mac sends ack it's unexpected, ignore.
            break

        default:
            ctrlPayloadContinuation?.yield(signed.payload)
        }
    }

    private func sendIce(_ candidateJson: String) async {
        guard let ws = ws else { return }
        let frame: [String: Any] = ["kind": "ice", "candidate": candidateJson]
        if let outer = try? JSONSerialization.data(withJSONObject: frame),
           let s = String(data: outer, encoding: .utf8) {
            try? await ws.send(s)
        }
    }

    private func fetchTurnCred() async -> [[String: Any]]? {
        let ts = UInt64(Date().timeIntervalSince1970 * 1000)
        guard let secret = Data(base64Encoded: pair.deviceSecretB64) else { return nil }
        let msg = "turn-cred|\(pair.pairId)|\(ts)"
        let sig = PairKeys.hmacSign(secret: secret, message: Data(msg.utf8)).base64EncodedString()
        var req = URLRequest(url: URL(string: "\(pair.workerURL)/turn/cred")!)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try? JSONSerialization.data(withJSONObject: [
            "pair_id": pair.pairId,
            "ts": ts,
            "sig": sig,
        ])
        guard let (data, _) = try? await URLSession.shared.data(for: req),
              let dict = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let arr = dict["ice_servers"] as? [[String: Any]] else { return nil }
        return arr
    }
}

// M6.6: InputClient sends ctrl via RtcGlue.
extension RtcGlue: InputClient.Glue {}
