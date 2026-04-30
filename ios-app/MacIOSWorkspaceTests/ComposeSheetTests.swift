import XCTest
@testable import MacIOSWorkspace
import SwiftUI

final class ComposeSheetTests: XCTestCase {
    func testOnSendReceivesPlainText() {
        var captured = ""
        let binding = Binding<String>(get: { "hello world" }, set: { _ in })
        let sheet = ComposeSheet(
            text: binding,
            title: "test",
            onSend: { captured = $0 },
            onCancel: { XCTFail("should not cancel") }
        )
        // ComposeSheet 是 SwiftUI View，直接调用闭包测试其语义契约
        sheet.onSend(binding.wrappedValue)
        XCTAssertEqual(captured, "hello world")
    }

    func testOnSendReceivesChineseAndEmoji() {
        var captured = ""
        let value = "你好 🦀 世界\nLine 2"
        let binding = Binding<String>(get: { value }, set: { _ in })
        let sheet = ComposeSheet(
            text: binding,
            title: "t",
            onSend: { captured = $0 },
            onCancel: {}
        )
        sheet.onSend(binding.wrappedValue)
        XCTAssertEqual(captured, value)
    }

    func testOnCancelDoesNotCallOnSend() {
        var sentCalled = false
        var cancelCalled = false
        let binding = Binding<String>(get: { "abc" }, set: { _ in })
        let sheet = ComposeSheet(
            text: binding,
            title: "t",
            onSend: { _ in sentCalled = true },
            onCancel: { cancelCalled = true }
        )
        sheet.onCancel()
        XCTAssertFalse(sentCalled)
        XCTAssertTrue(cancelCalled)
    }
}
