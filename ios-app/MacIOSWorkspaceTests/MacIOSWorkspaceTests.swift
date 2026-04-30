import XCTest
@testable import MacIOSWorkspace

final class MacIOSWorkspaceTests: XCTestCase {
    func testBundleIdentifierIsPresent() throws {
        let bundle = Bundle(for: type(of: self))
        XCTAssertNotNil(bundle.bundleIdentifier, "test bundle should have an identifier")
    }

    func testAppModuleLoads() throws {
        // 仅校验 @testable import 链路通畅，避免后续重构时悄悄掉了 target membership
        _ = ContentView()
    }
}
