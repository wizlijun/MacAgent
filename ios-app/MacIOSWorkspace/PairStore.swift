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

    func revoke() throws {
        try Keychain.delete("ios.pair.record")
        try Keychain.delete("ios.local.privkey")
        state = .unpaired
    }
}
