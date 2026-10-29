import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch

@MainActor
final class RetainedTextDisplayTests: XCTestCase {
    func testRefreshKeepsTextKit2RendererIntact() async throws {
        let text = RecordingTextView(usingTextLayoutManager: true)
        text.string = "Hi Kartikey 👋"
        let renderer = try XCTUnwrap(text.textLayoutManager)
        let host = TrackingHostingView(rootView: RetainedText(view: text))
        host.frame = CGRect(x: 0, y: 0, width: 440, height: 400)
        host.layoutSubtreeIfNeeded()
        text.displayInvalidations = 0
        host.scheduleTextDisplayRefresh()
        await drainMainQueue()
        XCTAssertTrue(text.textLayoutManager === renderer)
        XCTAssertEqual(text.string, "Hi Kartikey 👋")
        XCTAssertGreaterThan(text.displayInvalidations, 0)
    }

    func testRefreshInvalidatesRetainedTextWithoutReplacingContentOrSelection() async {
        let text = RecordingTextView(frame: CGRect(x: 0, y: 0, width: 300, height: 100))
        text.string = "Your very bottom WhatsApp chat is Group 1 - SST Class."
        text.isEditable = false
        text.isSelectable = true
        let selection = NSRange(location: 33, length: 19)
        text.setSelectedRange(selection)
        let host = TrackingHostingView(rootView: RetainedText(view: text))
        host.frame = CGRect(x: 0, y: 0, width: 440, height: 400)
        host.layoutSubtreeIfNeeded()
        let storage = text.textStorage
        host.isHidden = true
        host.frame.size = CGSize(width: 58, height: 30)
        host.isHidden = false
        host.frame.size = CGSize(width: 440, height: 400)
        // Let previously queued frame changes settle; this test never displays a window.
        await drainMainQueue()
        text.displayInvalidations = 0
        host.scheduleTextDisplayRefresh()
        await drainMainQueue()
        XCTAssertGreaterThan(text.displayInvalidations, 0)
        XCTAssertTrue(text.textStorage === storage)
        XCTAssertEqual(text.string, "Your very bottom WhatsApp chat is Group 1 - SST Class.")
        XCTAssertEqual(text.selectedRange(), selection)
        XCTAssertTrue(text.isSelectable)
    }
    private func drainMainQueue() async {
        await withCheckedContinuation { continuation in
            DispatchQueue.main.async { continuation.resume() }
        }
    }
}

private final class RecordingTextView: NSTextView {
    var displayInvalidations = 0
    override var needsDisplay: Bool {
        get { super.needsDisplay }
        set {
            if newValue { displayInvalidations += 1 }
            super.needsDisplay = newValue
        }
    }
}

private struct RetainedText: NSViewRepresentable {
    let view: RecordingTextView
    func makeNSView(context: Context) -> RecordingTextView { view }
    func updateNSView(_ nsView: RecordingTextView, context: Context) {}
}
