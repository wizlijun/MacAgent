import XCTest
@testable import MacIOSWorkspace

@MainActor
final class InputClientTests: XCTestCase {
    func testScrollThrottle() async {
        let stub = StubGlue()
        let client = InputClient(supId: "abc", glue: stub)
        for _ in 0..<5 {
            await client.scroll(dx: 0, dy: 1)
        }
        // 5 calls within < 16ms → 1 emission expected; allow up to 2 due to scheduler.
        XCTAssertLessThanOrEqual(stub.scrollSendCount, 2)
        XCTAssertGreaterThanOrEqual(stub.scrollSendCount, 1)
    }

    func testPasteThresholdShort() async {
        let stub = StubGlue()
        let client = InputClient(supId: "abc", glue: stub)
        await client.submitText("hello world")  // 11 chars
        XCTAssertEqual(stub.lastInputKind, "key_text")
        XCTAssertEqual(stub.clipboardSets, 0)
    }

    func testPasteThresholdLong() async {
        let stub = StubGlue()
        let client = InputClient(supId: "abc", glue: stub)
        let longText = String(repeating: "中", count: 50)
        await client.submitText(longText)
        XCTAssertEqual(stub.clipboardSets, 1)
        XCTAssertEqual(stub.lastInputKind, "key_combo")  // Cmd+V
    }
}

// Test stub that records ctrl messages instead of sending them.
final class StubGlue: InputClient.Glue, @unchecked Sendable {
    var scrollSendCount = 0
    var clipboardSets = 0
    var lastInputKind: String?

    func sendCtrl(_ payload: CtrlPayload) async {
        switch payload {
        case .guiInputCmd(_, let inner):
            switch inner {
            case .scroll: scrollSendCount += 1; lastInputKind = "scroll"
            case .tap: lastInputKind = "tap"
            case .keyText: lastInputKind = "key_text"
            case .keyCombo: lastInputKind = "key_combo"
            }
        case .clipboardSet:
            clipboardSets += 1
        default: break
        }
    }
}
