import XCTest
@testable import MacIOSWorkspace

final class PairKeysTests: XCTestCase {
    func testECDHRoundTrip() throws {
        let alice = PairKeys.generate()
        let bob = PairKeys.generate()
        let s1 = try alice.deriveSharedSecret(peerPubB64: bob.publicKeyB64)
        let s2 = try bob.deriveSharedSecret(peerPubB64: alice.publicKeyB64)
        XCTAssertEqual(s1, s2)
        XCTAssertEqual(s1.count, 32)
    }

    func testHMACSignVerify() throws {
        let secret = Data(repeating: 0xAB, count: 32)
        let sig = PairKeys.hmacSign(secret: secret, message: Data("hello".utf8))
        XCTAssertTrue(PairKeys.hmacVerify(secret: secret, message: Data("hello".utf8), sig: sig))
        XCTAssertFalse(PairKeys.hmacVerify(secret: secret, message: Data("hello!".utf8), sig: sig))
    }

    func testKeychainPersistence() throws {
        let key = "test.macagent.testpersist"
        try Keychain.set(key, value: Data("hello".utf8))
        let read = try Keychain.get(key)
        XCTAssertEqual(read, Data("hello".utf8))
        try Keychain.delete(key)
        XCTAssertNil(try Keychain.get(key))
    }

    func testCanonicalJSONSortedKeys() throws {
        let data = try CanonicalJSON.encode(["b": 1, "a": 2, "c": 3])
        let json = String(data: data, encoding: .utf8)!
        XCTAssertEqual(json, "{\"a\":2,\"b\":1,\"c\":3}")
    }
}
