import CryptoKit
import Foundation

struct PairKeys {
    let privateKey: Curve25519.KeyAgreement.PrivateKey
    var publicKeyData: Data { privateKey.publicKey.rawRepresentation }
    var publicKeyB64: String { publicKeyData.base64EncodedString() }
    var privateKeyData: Data { privateKey.rawRepresentation }

    static func generate() -> PairKeys {
        PairKeys(privateKey: Curve25519.KeyAgreement.PrivateKey())
    }

    static func from(privateKeyData: Data) throws -> PairKeys {
        let pk = try Curve25519.KeyAgreement.PrivateKey(rawRepresentation: privateKeyData)
        return PairKeys(privateKey: pk)
    }

    func deriveSharedSecret(peerPubB64: String) throws -> Data {
        guard let peerData = Data(base64Encoded: peerPubB64), peerData.count == 32 else {
            throw NSError(domain: "PairKeys", code: 1, userInfo: [NSLocalizedDescriptionKey: "bad peer pubkey"])
        }
        let peer = try Curve25519.KeyAgreement.PublicKey(rawRepresentation: peerData)
        let shared = try privateKey.sharedSecretFromKeyAgreement(with: peer)
        return shared.withUnsafeBytes { Data($0) }
    }

    static func hmacSign(secret: Data, message: Data) -> Data {
        let key = SymmetricKey(data: secret)
        let mac = HMAC<SHA256>.authenticationCode(for: message, using: key)
        return Data(mac)
    }

    static func hmacVerify(secret: Data, message: Data, sig: Data) -> Bool {
        let key = SymmetricKey(data: secret)
        return HMAC<SHA256>.isValidAuthenticationCode(sig, authenticating: message, using: key)
    }
}

enum Keychain {
    static let service = "com.hemory.macagent"

    static func set(_ key: String, value: Data) throws {
        let query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: key,
        ]
        SecItemDelete(query as CFDictionary)
        var add = query
        add[kSecValueData] = value
        add[kSecAttrAccessible] = kSecAttrAccessibleAfterFirstUnlock
        let st = SecItemAdd(add as CFDictionary, nil)
        if st != errSecSuccess { throw NSError(domain: "Keychain", code: Int(st)) }
    }

    static func get(_ key: String) throws -> Data? {
        let query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: key,
            kSecReturnData: true,
            kSecMatchLimit: kSecMatchLimitOne,
        ]
        var item: CFTypeRef?
        let st = SecItemCopyMatching(query as CFDictionary, &item)
        if st == errSecItemNotFound { return nil }
        if st != errSecSuccess { throw NSError(domain: "Keychain", code: Int(st)) }
        return item as? Data
    }

    static func delete(_ key: String) throws {
        let query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: key,
        ]
        let st = SecItemDelete(query as CFDictionary)
        if st != errSecSuccess && st != errSecItemNotFound {
            throw NSError(domain: "Keychain", code: Int(st))
        }
    }
}
