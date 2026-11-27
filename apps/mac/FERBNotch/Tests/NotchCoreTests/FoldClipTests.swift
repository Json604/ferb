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

    // decision: APP-815, APP-813, APP-725
    func testShowAllGrowsItsRowAndShowLessFoldsItWithNothingOverlapping() async throws {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults("FoldClipList"))
        model.preferredTranscriptHeight = 500
        var long = ConversationTurn(id: UUID(), createdAt: Date(), request: "the visa rules", sources: [])
        long.status = "done"
        long.answer = (1...80).map { "- 7.\($0) a long bullet about the visa rules and what to bring" }.joined(separator: "\n")
        var next = ConversationTurn(id: UUID(), createdAt: Date().addingTimeInterval(5), request: "and the fee?", sources: [])
        next.status = "done"
        next.answer = "The fee is 60 euros."
        let host = NSHostingView(rootView: ConversationTranscript(
            entries: [.turn(long, answeredBelow: false), .turn(next, answeredBelow: false)], model: model))
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 440, height: 500),
                              styleMask: .borderless, backing: .buffered, defer: true)
        window.contentView = host                                   // never ordered front or captured
        func settle() async {
            for _ in 0..<4 {
                host.layoutSubtreeIfNeeded()
                await withCheckedContinuation { continuation in DispatchQueue.main.async { continuation.resume() } }
            }
            host.layoutSubtreeIfNeeded()
        }
        func gap() throws -> (folded: Bool, answerBottom: CGFloat, nextTop: CGFloat) {
            let views = textViews(in: host)
            let answer = try XCTUnwrap(views.first { $0.string.hasPrefix("7.1") || $0.string.contains("7.1 a long") })
            let after = try XCTUnwrap(views.first { $0.string == "and the fee?" })
            // in the list's own (flipped) coordinates: where each stands in the transcript, wherever the view is
            let list = try XCTUnwrap(answer.enclosingScrollView?.documentView)
            let a = answer.convert(answer.bounds, to: list), b = after.convert(after.bounds, to: list)
            return (answer.shownHeight != nil, a.maxY, b.minY)
        }
        await settle()
        let folded = try gap()
        XCTAssertTrue(folded.folded, "the long answer is folded")
        XCTAssertLessThanOrEqual(folded.answerBottom, folded.nextTop, "nothing of the folded answer reaches the next entry")

        model.toggleExpanded(ExpansionState.key(turn: long.id, item: "answer"), target: TranscriptTarget.answer(turn: long.id))
        await settle()
        // the text opened downwards from where it stood (owner 2026-11-27): the next entry is brought into view to look
        model.requestScroll(TranscriptTarget.entry(next.id), .collapse)
        await settle()
        let open = try gap()
        XCTAssertFalse(open.folded, "Show all shows all of it")
        XCTAssertLessThanOrEqual(open.answerBottom, open.nextTop, "its row grew with it: the next entry is below it")
        XCTAssertGreaterThan(open.nextTop - folded.nextTop, 300, "the next entry stands lower in the transcript by what was shown")

        model.toggleExpanded(ExpansionState.key(turn: long.id, item: "answer"), target: TranscriptTarget.answer(turn: long.id))
        await settle()
        let again = try gap()
        XCTAssertTrue(again.folded)
        XCTAssertLessThanOrEqual(again.answerBottom, again.nextTop)
        withExtendedLifetime(window) {}
    }

    /// Owner 2026-11-14, a five-step plan shown whole past the chat's height: "why doesnt it follow the show more logic we
    /// have for other things??? IT SHOULD FOLLOW IT". The plan bubble folds by the long answer's rule and component.
    // decision: APP-911, APP-725
    func testALongPlanFoldsAsALongAnswerDoesAndAShortOneShowsWhole() async throws {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults("FoldPlan"))
        model.preferredTranscriptHeight = 300
        func planned(_ request: String, steps: Int, at offset: TimeInterval) -> ConversationTurn {
            var turn = ConversationTurn(id: UUID(), createdAt: Date().addingTimeInterval(offset), request: request, sources: [])
            turn.status = "working"
            turn.planLine = "Here's the plan; I'm starting now. Reply to change anything:"
            turn.planLines = (1...steps).map { "- Step \($0): using the chosen browser profile and the earlier research, "
                + "make the document the owner asked for and give its link." }
            return turn
        }
        let long = planned("five things at once", steps: 24, at: 0)
        let short = planned("one thing", steps: 2, at: 5)
        let host = NSHostingView(rootView: ConversationTranscript(
            entries: [.turn(long, answeredBelow: false), .turn(short, answeredBelow: false)], model: model))
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 440, height: 300),
                              styleMask: .borderless, backing: .buffered, defer: true)
        window.contentView = host                                   // never ordered front or captured
        func settle() async {
            for _ in 0..<4 {
                host.layoutSubtreeIfNeeded()
                await withCheckedContinuation { continuation in DispatchQueue.main.async { continuation.resume() } }
            }
            host.layoutSubtreeIfNeeded()
        }
        func plan(_ steps: Int) throws -> MessageTextView {
            try XCTUnwrap(textViews(in: host).first { $0.string.contains("Step \(steps): using") && !$0.string.contains("Step \(steps + 1):") })
        }
        // the long one is built when the view is at its row
        await settle()
        model.requestScroll(TranscriptTarget.entry(long.id), .collapse)
        await settle()
        let folded = try plan(24)
        XCTAssertNotNil(folded.shownHeight, "a plan taller than the chat folds")
        XCTAssertLessThan(folded.bounds.height, folded.measuredSize(width: folded.bounds.width).height / 2)
        XCTAssertTrue(folded.string.contains("Step 24"), "the same text, the rest one click away")

        model.toggleExpanded(ExpansionState.key(turn: long.id, item: "plan"), target: TranscriptTarget.entry(long.id))
        await settle()
        let open = try plan(24)
        XCTAssertNil(open.shownHeight, "Show all shows every step")
        XCTAssertGreaterThanOrEqual(open.bounds.height + 1, open.measuredSize(width: open.bounds.width).height)

        model.requestScroll(TranscriptTarget.entry(short.id), .collapse)
        await settle()
        XCTAssertNil(try plan(2).shownHeight, "a short plan shows whole, with no control")
        withExtendedLifetime(window) {}
    }

    private func textViews(in view: NSView) -> [MessageTextView] {
        if let field = view as? MessageTextView { return [field] }
        return view.subviews.flatMap { textViews(in: $0) }
    }
}
