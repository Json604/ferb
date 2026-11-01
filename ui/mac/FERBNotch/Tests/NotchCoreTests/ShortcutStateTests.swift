import XCTest
import AppKit
@testable import NotchCore
@testable import FERBNotch

final class ShortcutStateTests: XCTestCase {
    @MainActor
    // decision: APP-552
    func testNativeModifierFlagsPreserveTheCommandKeySide() {
        let right = NSEvent.ModifierFlags(rawValue: NSEvent.ModifierFlags.command.rawValue
                                          | NSEvent.ModifierFlags.option.rawValue | 0x10 | 0x40)
        let left = NSEvent.ModifierFlags(rawValue: NSEvent.ModifierFlags.command.rawValue
                                         | NSEvent.ModifierFlags.option.rawValue | 0x08 | 0x40)
        XCTAssertEqual(NotchShortcutMonitor.modifiers(from: right), [.rightCommand, .rightOption])
        XCTAssertEqual(NotchShortcutMonitor.modifiers(from: left), [.leftCommand, .rightOption])
    }

    // decision: APP-552
    func testRightOptionDoubleTapOnlyOpensTyping() {
        var state = NotchShortcutState()
        XCTAssertEqual(state.update([.rightOption], at: 0, keyCount: 0), [])
        XCTAssertEqual(state.update([], at: 0.1, keyCount: 0), [])
        XCTAssertEqual(state.update([.rightOption], at: 0.2, keyCount: 0), [])
        XCTAssertEqual(state.update([], at: 0.3, keyCount: 0), [.togglePanel])
    }

    // decision: APP-552
    func testHoldingOptionAloneNeverStartsVoice() {
        var state = NotchShortcutState()
        XCTAssertEqual(state.update([.rightOption], at: 0, keyCount: 0), [])
        XCTAssertEqual(state.update([], at: 3, keyCount: 0), [])
    }

    // decision: APP-552
    func testVoiceStartsImmediatelyInEitherPressOrderAndEndsOnce() {
        for option: ShortcutModifiers in [.rightOption, .leftOption] {
            for first: ShortcutModifiers in [option, .rightCommand] {
                var state = NotchShortcutState()
                let chord: ShortcutModifiers = [option, .rightCommand]
                XCTAssertEqual(state.update(first, at: 0, keyCount: 0), [])
                XCTAssertEqual(state.update(chord, at: 0.01, keyCount: 0), [.startVoice])
                XCTAssertEqual(state.update(chord, at: 0.01, keyCount: 0), [])
                XCTAssertEqual(state.update(first, at: 1, keyCount: 0), [.finishVoice])
                XCTAssertEqual(state.update([], at: 1.1, keyCount: 0), [])
            }
        }
    }

    // decision: APP-552
    func testLeftCommandAndOptionDoNotStartVoice() {
        var state = NotchShortcutState()
        XCTAssertEqual(state.update([.leftCommand, .rightOption], at: 0, keyCount: 0), [])
        XCTAssertEqual(state.update([], at: 1, keyCount: 0), [])
    }

    // decision: APP-552
    func testAnOrdinaryKeyboardChordCancelsVoiceAndCannotBecomeADoubleTap() {
        var state = NotchShortcutState()
        _ = state.update([.rightOption], at: 0, keyCount: 0)
        XCTAssertEqual(state.update([.rightOption, .rightCommand], at: 0.1, keyCount: 0), [.startVoice])
        XCTAssertEqual(state.update([.rightOption], at: 0.2, keyCount: 1), [.cancelVoice])
        XCTAssertEqual(state.update([], at: 0.21, keyCount: 1), [])
        XCTAssertEqual(state.update([.rightOption], at: 0.3, keyCount: 1), [])
        XCTAssertEqual(state.update([], at: 0.4, keyCount: 1), [])
    }

    // decision: APP-552
    func testAdditionalModifierCancelsVoiceWithoutRestartingUntilReleased() {
        var state = NotchShortcutState()
        XCTAssertEqual(state.update([.rightOption, .rightCommand], at: 0, keyCount: 0), [.startVoice])
        XCTAssertEqual(state.update([.rightOption, .rightCommand, .shift], at: 0.1, keyCount: 0), [.cancelVoice])
        XCTAssertEqual(state.update([.rightOption, .rightCommand], at: 0.2, keyCount: 0), [])
        XCTAssertEqual(state.update([], at: 0.3, keyCount: 0), [])
        XCTAssertEqual(state.update([.rightOption, .rightCommand], at: 0.4, keyCount: 0), [.startVoice])
        XCTAssertEqual(state.reset(), [.cancelVoice])
    }
}
