import Foundation
import Observation

@Observable
final class PairStore {
    enum State { case unpaired, paired(PairedPair) }

    struct PairedPair: Codable, Equatable {
        let pairId: String
        let peerPubB64: String
        let deviceSecretB64: String
        let workerURL: String
    }

    private(set) var state: State

    init() {
        if let data = try? Keychain.get("ios.pair.record"),
           let pair = try? JSONDecoder().decode(PairedPair.self, from: data) {
            state = .paired(pair)
        } else {
            state = .unpaired
        }
    }

    func savePair(_ pair: PairedPair) throws {
        let data = try JSONEncoder().encode(pair)
        try Keychain.set("ios.pair.record", value: data)
        state = .paired(pair)
    }

    func revoke() async {
        if case let .paired(pair) = state {
            let ts = UInt64(Date().timeIntervalSince1970 * 1000)
            if let secret = Data(base64Encoded: pair.deviceSecretB64) {
                let msg = "revoke|\(pair.pairId)|\(ts)"
                let sig = PairKeys.hmacSign(secret: secret, message: Data(msg.utf8)).base64EncodedString()
                var req = URLRequest(url: URL(string: "\(pair.workerURL)/pair/revoke")!)
                req.httpMethod = "POST"
                req.setValue("application/json", forHTTPHeaderField: "Content-Type")
                req.httpBody = try? JSONSerialization.data(withJSONObject: [
                    "pair_id": pair.pairId, "ts": ts, "sig": sig,
                ])
                // best-effort: failure does not block local cleanup
                _ = try? await URLSession.shared.data(for: req)
            }
        }
        try? Keychain.delete("ios.pair.record")
        try? Keychain.delete("ios.local.privkey")
        state = .unpaired
    }
}
