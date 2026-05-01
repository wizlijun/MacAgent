import Foundation

actor SignalingClient {
    private let task: URLSessionWebSocketTask

    init(workerURL: String, pairID: String, deviceSecret: Data) throws {
        let ts = UInt64(Date().timeIntervalSince1970 * 1000)
        let nonceBytes = (0..<16).map { _ in UInt8.random(in: 0...255) }
        let nonceB64 = Data(nonceBytes).base64EncodedString()
        let sigMsg = "ws-auth|ios|\(pairID)|\(ts)|\(nonceB64)"
        let sigData = PairKeys.hmacSign(secret: deviceSecret, message: Data(sigMsg.utf8))
        let sigB64 = sigData.base64EncodedString()

        guard var c = URLComponents(string: "\(workerURL)/signal/\(pairID)") else {
            throw NSError(domain: "Sig", code: 1, userInfo: [NSLocalizedDescriptionKey: "bad worker URL"])
        }
        c.scheme = c.scheme == "https" ? "wss" : "ws"
        // URLComponents.queryItems 不会编码 base64 里的 +、/、=（在 RFC 3986 里合法但
        // URLSearchParams 会把 + 解码成空格），用 percentEncodedQueryItems 手动转义。
        let sigEnc = SignalingClient.b64ForQuery(sigB64)
        let nonceEnc = SignalingClient.b64ForQuery(nonceB64)
        c.percentEncodedQueryItems = [
            .init(name: "device", value: "ios"),
            .init(name: "pair_id", value: pairID),
            .init(name: "ts", value: "\(ts)"),
            .init(name: "nonce", value: nonceEnc),
            .init(name: "sig", value: sigEnc),
        ]
        guard let url = c.url else { throw NSError(domain: "Sig", code: 2) }
        task = URLSession.shared.webSocketTask(with: url)
        task.resume()
    }

    func send(_ json: String) async throws {
        try await task.send(.string(json))
    }

    func recv() async throws -> String {
        switch try await task.receive() {
        case .string(let s): return s
        case .data: throw NSError(domain: "Sig", code: 3)
        @unknown default: throw NSError(domain: "Sig", code: 4)
        }
    }

    func close() {
        task.cancel(with: .normalClosure, reason: nil)
    }

    /// Percent-encode +, /, = so query parsers using application/x-www-form-urlencoded
    /// (e.g. URLSearchParams) don't turn `+` into a space in the base64 payload.
    static func b64ForQuery(_ s: String) -> String {
        return s
            .replacingOccurrences(of: "+", with: "%2B")
            .replacingOccurrences(of: "/", with: "%2F")
            .replacingOccurrences(of: "=", with: "%3D")
    }
}
