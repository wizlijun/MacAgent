import Foundation

enum CanonicalJSON {
    static func encode(_ obj: [String: Any]) throws -> Data {
        return try JSONSerialization.data(
            withJSONObject: obj, options: [.sortedKeys, .withoutEscapingSlashes]
        )
    }
}
