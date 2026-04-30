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
        c.queryItems = [
            .init(name: "device", value: "ios"),
            .init(name: "pair_id", value: pairID),
            .init(name: "ts", value: "\(ts)"),
            .init(name: "nonce", value: nonceB64),
            .init(name: "sig", value: sigB64),
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
}
