import Foundation

enum CtrlPayload: Codable, Equatable {
    case ping(ts: UInt64, nonce: String)
    case pong(ts: UInt64, nonce: String)
    case heartbeat(ts: UInt64, nonce: String)
    case heartbeatAck(ts: UInt64, nonce: String)
    case error(code: String, msg: String)

    private enum CodingKeys: String, CodingKey { case type, ts, nonce, code, msg }

    func canonicalBytes() throws -> Data {
        switch self {
        case .ping(let ts, let nonce):
            return try CanonicalJSON.encode(["type": "ping", "ts": ts, "nonce": nonce])
        case .pong(let ts, let nonce):
            return try CanonicalJSON.encode(["type": "pong", "ts": ts, "nonce": nonce])
        case .heartbeat(let ts, let nonce):
            return try CanonicalJSON.encode(["type": "heartbeat", "ts": ts, "nonce": nonce])
        case .heartbeatAck(let ts, let nonce):
            return try CanonicalJSON.encode(["type": "heartbeat_ack", "ts": ts, "nonce": nonce])
        case .error(let code, let msg):
            return try CanonicalJSON.encode(["type": "error", "code": code, "msg": msg])
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .ping(let ts, let nonce):
            try c.encode("ping", forKey: .type)
            try c.encode(ts, forKey: .ts); try c.encode(nonce, forKey: .nonce)
        case .pong(let ts, let nonce):
            try c.encode("pong", forKey: .type)
            try c.encode(ts, forKey: .ts); try c.encode(nonce, forKey: .nonce)
        case .heartbeat(let ts, let nonce):
            try c.encode("heartbeat", forKey: .type)
            try c.encode(ts, forKey: .ts); try c.encode(nonce, forKey: .nonce)
        case .heartbeatAck(let ts, let nonce):
            try c.encode("heartbeat_ack", forKey: .type)
            try c.encode(ts, forKey: .ts); try c.encode(nonce, forKey: .nonce)
        case .error(let code, let msg):
            try c.encode("error", forKey: .type)
            try c.encode(code, forKey: .code); try c.encode(msg, forKey: .msg)
        }
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let type = try c.decode(String.self, forKey: .type)
        switch type {
        case "ping":
            self = .ping(ts: try c.decode(UInt64.self, forKey: .ts),
                         nonce: try c.decode(String.self, forKey: .nonce))
        case "pong":
            self = .pong(ts: try c.decode(UInt64.self, forKey: .ts),
                         nonce: try c.decode(String.self, forKey: .nonce))
        case "heartbeat":
            self = .heartbeat(ts: try c.decode(UInt64.self, forKey: .ts),
                              nonce: try c.decode(String.self, forKey: .nonce))
        case "heartbeat_ack":
            self = .heartbeatAck(ts: try c.decode(UInt64.self, forKey: .ts),
                                 nonce: try c.decode(String.self, forKey: .nonce))
        case "error":
            self = .error(code: try c.decode(String.self, forKey: .code),
                          msg: try c.decode(String.self, forKey: .msg))
        default:
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "unknown type \(type)"))
        }
    }
}

struct SignedCtrl: Codable {
    let payload: CtrlPayload
    let sig: String

    enum CodingKeys: String, CodingKey { case sig }

    static func sign(_ p: CtrlPayload, sharedSecret: Data) throws -> SignedCtrl {
        let bytes = try p.canonicalBytes()
        let sig = PairKeys.hmacSign(secret: sharedSecret, message: bytes).base64EncodedString()
        return SignedCtrl(payload: p, sig: sig)
    }

    func verify(sharedSecret: Data) throws {
        guard let sigBytes = Data(base64Encoded: sig) else {
            throw NSError(domain: "Ctrl", code: 1)
        }
        guard PairKeys.hmacVerify(secret: sharedSecret, message: try payload.canonicalBytes(), sig: sigBytes) else {
            throw NSError(domain: "Ctrl", code: 1, userInfo: [NSLocalizedDescriptionKey: "bad sig"])
        }
    }

    func encode(to encoder: Encoder) throws {
        // 把 payload 字段平铺到顶层，再加 sig
        try payload.encode(to: encoder)
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(sig, forKey: .sig)
    }

    init(from decoder: Decoder) throws {
        let payload = try CtrlPayload(from: decoder)
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let sig = try c.decode(String.self, forKey: .sig)
        self.init(payload: payload, sig: sig)
    }

    init(payload: CtrlPayload, sig: String) {
        self.payload = payload
        self.sig = sig
    }
}
