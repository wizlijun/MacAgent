import SwiftUI

struct PairedView: View {
    let pair: PairStore.PairedPair
    @State var store: PairStore
    @State var pingResult: String?
    @State var pinging = false

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: "checkmark.seal.fill")
                .resizable().scaledToFit().frame(maxWidth: 80).foregroundStyle(.green)
            Text("已配对").font(.title.bold())
            Text("pair_id: \(pair.pairId.prefix(8))…").font(.caption).foregroundStyle(.secondary)

            Button(action: { Task { await ping() } }) {
                if pinging { ProgressView() } else { Text("发送 ping 测试") }
            }
            .buttonStyle(.bordered)
            .disabled(pinging)

            if let r = pingResult { Text(r).font(.caption.monospaced()).multilineTextAlignment(.center) }

            Button("撤销并重新配对") { try? store.revoke() }.buttonStyle(.bordered).tint(.red)
        }
        .padding()
    }

    private func ping() async {
        pinging = true
        defer { pinging = false }
        do {
            guard let priv = try Keychain.get("ios.local.privkey") else {
                pingResult = "ERR: 找不到本地私钥"; return
            }
            let keys = try PairKeys.from(privateKeyData: priv)
            let sharedSecret = try keys.deriveSharedSecret(peerPubB64: pair.peerPubB64)
            guard let secret = Data(base64Encoded: pair.deviceSecretB64) else {
                pingResult = "ERR: device_secret 解码失败"; return
            }
            let client = try SignalingClient(workerURL: pair.workerURL, pairID: pair.pairId, deviceSecret: secret)
            let nonce = "ios-\(UUID().uuidString.prefix(8))"
            let ts = UInt64(Date().timeIntervalSince1970 * 1000)
            let signed = try SignedCtrl.sign(.ping(ts: ts, nonce: String(nonce)), sharedSecret: sharedSecret)
            let json = String(data: try JSONEncoder().encode(signed), encoding: .utf8)!
            try await client.send(json)
            let resp = try await client.recv()
            let echoed = try JSONDecoder().decode(SignedCtrl.self, from: Data(resp.utf8))
            try echoed.verify(sharedSecret: sharedSecret)
            pingResult = "OK：收到 \(echoed.payload)"
            await client.close()
        } catch {
            pingResult = "ERR: \(error)"
        }
    }
}
