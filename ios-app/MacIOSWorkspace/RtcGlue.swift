import Foundation

enum GlueState: Equatable { case idle, fetchingTurn, signalingConnected, negotiating, connected, failed }

actor RtcGlue {
    private let pair: PairStore.PairedPair
    private var rtc: RtcClient?
    private var ws: SignalingClient?
    private var stateContinuation: AsyncStream<GlueState>.Continuation?

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

    private func emit(_ s: GlueState) {
        stateContinuation?.yield(s)
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
                self.handleState(state)
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
            default:
                break
            }
        }
    }

    private func sendIce(_ candidateJson: String) async {
        guard let ws = ws else { return }
        guard let data = candidateJson.data(using: .utf8),
              var inner = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
        inner["kind"] = "ice"
        if let outer = try? JSONSerialization.data(withJSONObject: inner),
           let s = String(data: outer, encoding: .utf8) {
            try? await ws.send(s)
        }
    }

    private func handleState(_ s: RtcClient.PeerState) {
        switch s {
        case .connected: emit(.connected)
        case .failed, .closed: emit(.failed)
        default: emit(.negotiating)
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
