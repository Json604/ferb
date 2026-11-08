import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch

/// Owner, 2026-11-08: "another bug i noticed, i cant open my onemeny clipboard when settings window is open". A secure
/// field holding the keyboard turns on secure event input for the whole Mac, which hides keys from other apps'
/// watchers; FERB's window gives one the keyboard only when the owner clicks it or tabs to it.
@MainActor
final class SecureEntryFocusTests: XCTestCase {
    private struct Entry: View {
        @State var key = ""
        @State var name = ""
        var body: some View {
            VStack {
                SecureField("Paste your API key", text: $key)
                TextField("Name", text: $name)
            }.frame(width: 300, height: 120)
        }
    }

    private func fields(_ view: NSView) -> [NSTextField] {
        ((view as? NSTextField).map { [$0] } ?? []) + view.subviews.flatMap(fields)
    }

    override func tearDown() {
        SecureEntryFocus.currentEvent = { NSApp.currentEvent }
        super.tearDown()
    }

    // decision: APP-794, APP-795, APP-790
    func testASecureFieldTakesTheKeyboardOnlyFromTheOwnersClickOrTab() throws {
        _ = NSApplication.shared
        let window = FERBAppWindow(contentRect: NSRect(x: 0, y: 0, width: 320, height: 140), styleMask: [.titled],
                                   backing: .buffered, defer: false)
        window.contentView = NSHostingView(rootView: Entry())
        window.orderFrontRegardless()
        defer { window.orderOut(nil) }
        window.contentView?.layoutSubtreeIfNeeded()
        let all = fields(try XCTUnwrap(window.contentView))
        let secure = try XCTUnwrap(all.compactMap { $0 as? NSSecureTextField }.first)
        let plain = try XCTUnwrap(all.first { !($0 is NSSecureTextField) && $0.isEditable })

        // nothing the owner did: opening the window or a page never puts the keyboard in it
        SecureEntryFocus.currentEvent = { nil }
        XCTAssertFalse(window.makeFirstResponder(secure))
        XCTAssertNil(SecureEntryFocus.secureField(window.firstResponder))
        XCTAssertTrue(window.makeFirstResponder(plain))                 // an ordinary field is unchanged
        // a press elsewhere in the window, or a key other than Tab, isn't choosing it either
        let outside = NSPoint(x: 1, y: 1)
        SecureEntryFocus.currentEvent = { Self.press(at: outside, in: window) }
        XCTAssertFalse(window.makeFirstResponder(secure))
        SecureEntryFocus.currentEvent = { Self.key(36, in: window) }
        XCTAssertFalse(window.makeFirstResponder(secure))
        // the owner's click inside it, or Tab to it
        let inside = secure.convert(NSPoint(x: secure.bounds.midX, y: secure.bounds.midY), to: nil)
        SecureEntryFocus.currentEvent = { Self.press(at: inside, in: window) }
        XCTAssertTrue(window.makeFirstResponder(secure))
        XCTAssertTrue(window.makeFirstResponder(plain))
        SecureEntryFocus.currentEvent = { Self.key(SecureEntryFocus.tabKey, in: window) }
        XCTAssertTrue(window.makeFirstResponder(secure))
    }

    // decision: APP-790
    func testTheMenuWatchListsOnlyFERBsOwnWindows() {
        _ = NSApplication.shared
        // its default reads FERB's own window list: another app's window or menu is never in it, so never moved
        let own = Set(NSApp.windows.map(ObjectIdentifier.init))
        XCTAssertTrue(AppWindowPresentation.menuWindows().allSatisfy { own.contains(ObjectIdentifier($0)) })
    }

    private static func press(at point: NSPoint, in window: NSWindow) -> NSEvent? {
        NSEvent.mouseEvent(with: .leftMouseDown, location: point, modifierFlags: [], timestamp: 0,
                           windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1,
                           pressure: 1)
    }

    private static func key(_ code: UInt16, in window: NSWindow) -> NSEvent? {
        NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
                         windowNumber: window.windowNumber, context: nil, characters: "\t",
                         charactersIgnoringModifiers: "\t", isARepeat: false, keyCode: code)
    }
}
