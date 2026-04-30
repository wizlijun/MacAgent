import Foundation

struct PairTokenPayload: Codable {
    let pair_token: String
    let room_id: String
    let worker_url: String
}

enum PairingFlow {
    static func claim(scannedJSON: String, store: PairStore, apnsTokenHex: String? = nil) async throws {
        let token = try JSONDecoder().decode(PairTokenPayload.self, from: Data(scannedJSON.utf8))

        let keys: PairKeys
        if let priv = try Keychain.get("ios.local.privkey") {
            keys = try PairKeys.from(privateKeyData: priv)
        } else {
            keys = PairKeys.generate()
            try Keychain.set("ios.local.privkey", value: keys.privateKeyData)
        }

        var req = URLRequest(url: URL(string: "\(token.worker_url)/pair/claim")!)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        var body: [String: Any] = [
            "pair_token": token.pair_token,
            "ios_pubkey": keys.publicKeyB64,
        ]
        if let hex = apnsTokenHex {
            body["ios_apns_token"] = hex
        }
        req.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard (resp as? HTTPURLResponse)?.statusCode == 200 else {
            throw NSError(domain: "Pair", code: 1, userInfo: [NSLocalizedDescriptionKey: "claim failed"])
        }
        struct ClaimResp: Codable { let pair_id: String; let mac_pubkey: String; let ios_device_secret: String }
        let claim = try JSONDecoder().decode(ClaimResp.self, from: data)

        try store.savePair(.init(
            pairId: claim.pair_id,
            peerPubB64: claim.mac_pubkey,
            deviceSecretB64: claim.ios_device_secret,
            workerURL: token.worker_url
        ))
    }
}
