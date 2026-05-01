import XCTest
import UIKit
@testable import MacIOSWorkspace

final class KeyMapperTests: XCTestCase {
    func testModifierFlags() {
        XCTAssertEqual(KeyMapper.modifiers(from: [.command]), [.cmd])
        XCTAssertEqual(KeyMapper.modifiers(from: [.command, .shift]), [.cmd, .shift])
        XCTAssertEqual(KeyMapper.modifiers(from: [.alternate, .control]), [.opt, .ctrl])
        XCTAssertEqual(KeyMapper.modifiers(from: []), [])
    }

    func testNamedKeys() {
        XCTAssertEqual(KeyMapper.name(for: .keyboardEscape), "esc")
        XCTAssertEqual(KeyMapper.name(for: .keyboardTab), "tab")
        XCTAssertEqual(KeyMapper.name(for: .keyboardReturnOrEnter), "return")
        XCTAssertEqual(KeyMapper.name(for: .keyboardDeleteOrBackspace), "delete")
        XCTAssertEqual(KeyMapper.name(for: .keyboardUpArrow), "up")
        XCTAssertEqual(KeyMapper.name(for: .keyboardDownArrow), "down")
        XCTAssertEqual(KeyMapper.name(for: .keyboardLeftArrow), "left")
        XCTAssertEqual(KeyMapper.name(for: .keyboardRightArrow), "right")
        XCTAssertEqual(KeyMapper.name(for: .keyboardSpacebar), "space")
    }

    func testIsSpecial() {
        XCTAssertTrue(KeyMapper.isSpecial(.keyboardEscape))
        XCTAssertTrue(KeyMapper.isSpecial(.keyboardUpArrow))
        XCTAssertFalse(KeyMapper.isSpecial(.keyboardA))
    }
}
