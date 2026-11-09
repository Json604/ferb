import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Past-bug guard (TEST-503): with the fold inside the text view (APP-809) a folded answer's row was its folded height,
/// but the text view drew all of its text: since macOS 14 a view doesn't clip to its bounds unless asked, so the hidden
/// lines painted over the rows below it (owner 2026-11-09, in the notch and in History, every day). A folded text draws
/// nothing outside its own frame. The windows here are never ordered front.
@MainActor
final class FoldClipTests: XCTestCase {
    // decision: APP-813
    func testAFoldedAnswerDrawsNothingOutsideItsRow() async throws {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults("FoldClip"))
        model.preferredTranscriptHeight = 300
        let long = (1...80).map { "- 7.\($0) a long bullet about the visa rules and what to bring along" }
            .joined(separator: "\n")
        let root = VStack(alignment: .leading, spacing: 10) {
            FerbBubble {
                CollapsibleText(key: "k", kind: .answer(viewport: 300), target: "t", more: "Show all", model: model) {
                    MarkdownContent(long).equatable()
                }
            }
            Color.clear.frame(height: 900)
        }.frame(width: 420, alignment: .topLeading)
        let host = NSHostingView(rootView: root)
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 420, height: 1600),
                              styleMask: .borderless, backing: .buffered, defer: true)
        window.contentView = host                                   // never ordered front or captured
        for _ in 0..<3 {
            host.layoutSubtreeIfNeeded()
            await withCheckedContinuation { continuation in DispatchQueue.main.async { continuation.resume() } }
        }
        let text = try XCTUnwrap(textViews(in: host).first)
        XCTAssertNotNil(text.shownHeight, "the long answer is folded")
        let frame = text.convert(text.bounds, to: host)
        XCTAssertLessThan(frame.height, text.measuredSize(width: frame.width).height / 2, "its row is the folded height")
        XCTAssertTrue(text.clipsToBounds, "a folded text clips to its frame")

        let rep = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: rep)
        let scale = CGFloat(rep.pixelsHigh) / host.bounds.height
        // bitmap rows run top-down; the host is flipped like the text view
        let below = (host.isFlipped ? frame.maxY : host.bounds.height - frame.minY) + 12
        var most: CGFloat = 0
        for y in stride(from: Int(below * scale), to: min(rep.pixelsHigh, Int((below + 600) * scale)), by: 1) {
            for x in stride(from: 0, to: rep.pixelsWide, by: 2) {
                most = max(most, rep.colorAt(x: x, y: y)?.alphaComponent ?? 0)
            }
        }
        XCTAssertEqual(most, 0, "nothing of the folded text draws below its row")
        withExtendedLifetime(window) {}
    }

    private func textViews(in view: NSView) -> [MessageTextView] {
        if let field = view as? MessageTextView { return [field] }
        return view.subviews.flatMap { textViews(in: $0) }
    }
}
