import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Past-bug guard (TEST-503): a card row that failed while the notch was closed showed an empty body until clicked,
/// because it drew as SwiftUI's selectable `Text`, which the reveal refresh never reaches (758ce419 fixed it once; its
/// guard was deleted and 68c423e8 brought SwiftUI text back). All chat text draws through FERB's own text view.
/// The windows here are never ordered front.
@MainActor
final class ChatTextRevealTests: XCTestCase {
    private let failedText = "Couldn't set up the secure login: the site asked for a passkey this Mac doesn't have."

    // decision: APP-788
    func testARowThatFailedWhileClosedDrawsAgainAtTheReveal() async throws {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults("ChatTextReveal"))
        model.session.draft = "set up the secure login and read my mail"
        let id = model.session.beginWork()!
        model.conversation.begin(id: id, request: "set up the secure login and read my mail", sources: [])
        model.receiveChatEvent(["type": "plan", "request_id": "R", "task_id": "T", "goal": "login and mail",
                                "text": "I'll set it up.", "plan_line": "I'll set it up.", "lines": [],
                                "card": ["request_id": "R", "done": 0, "total": 2, "rows": [
                                    ["part": "login", "label": "Secure login setup", "state": "working", "step": "login"],
                                    ["part": "mail", "label": "Mail", "state": "working", "step": "mail"]]]],
                               localID: id)
        let long = (1...24).map { "Mail \($0): a long line about what that message said and who sent it." }
            .joined(separator: "\n\n")
        part("mail", state: "done", text: long, model: model)

        let host = NSHostingView(rootView: TranscriptHost(model: model))
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 440, height: 500),
                              styleMask: .borderless, backing: .buffered, defer: true)
        window.contentView = host                                   // never ordered front or captured
        await settle(host)
        XCTAssertTrue(textViews(in: host).contains { $0.shownHeight != nil },
                      "the long done row is collapsed by its own text view, not a SwiftUI mask")

        // the notch closes; the row fails while it is closed
        model.noteTextPresentation(expanded: false)
        window.setContentSize(CGSize(width: 58, height: 30))
        await settle(host)
        part("login", state: "failed", text: failedText, model: model)
        await settle(host)

        // it opens again; the reveal finishes
        window.setContentSize(CGSize(width: 440, height: 500))
        model.noteTextPresentation(expanded: true)
        await settle(host)
        let before = Dictionary(uniqueKeysWithValues: textViews(in: host).map { (ObjectIdentifier($0), $0.displayRefreshCount) })
        model.completeTextReveal()
        await settle(host)

        let shown = textViews(in: host)
        XCTAssertTrue(shown.contains { $0.string == failedText }, "the failed row is FERB's own text view")
        XCTAssertFalse(hasSwiftUISelectableText(in: host), "no SwiftUI selectable text in the chat")
        XCTAssertFalse(shown.isEmpty)
        for view in shown {
            let earlier = try XCTUnwrap(before[ObjectIdentifier(view)], "the same views across the reveal")
            XCTAssertGreaterThan(view.displayRefreshCount, earlier, "the reveal redraws \(view.string.prefix(30))")
        }
        withExtendedLifetime(window) {}
    }

    // decision: APP-788
    func testNoChatViewUsesSwiftUITextSelection() async throws {
        // what draws: failed, won't-run and done rows, the footer, a collapsed answer with its own lines, typed cards
        // and a question with its why line, exact box and evidence
        var cardTurn = ConversationTurn(id: UUID(), createdAt: Date(), request: "two things", sources: [])
        cardTurn.status = "done"
        cardTurn.card = TurnCard(["request_id": "R1", "done": 1, "total": 3, "line": "1 of 3 done",
                                  "lines": ["Apps used: Safari"],
                                  "rows": [["part": "a", "label": "Login", "state": "failed", "text": failedText, "step": "a"],
                                           ["part": "b", "label": "Upload", "state": "failed", "step": "b"],
                                           ["part": "c", "label": "Mail", "state": "done", "text": "Three mails.", "step": "c"]],
                                  "steps": ["b": ["status": "wont_run", "seq": "2026-11-07T10:00:00+00:00"]]],
                                 taskID: "T1")
        var answered = ConversationTurn(id: UUID(), createdAt: Date(), request: "a long answer", sources: [])
        answered.status = "done"
        answered.answer = (1...60).map { "Line \($0) of a long answer." }.joined(separator: "\n\n")
        answered.note = "the second source didn't load"
        answered.codeLines = ["Files changed: 2"]
        answered.cards = [ResultCard(["type": "note", "item_id": "m-1", "text": "my dentist is Dr. Rao", "state": "saved"])!,
                          ResultCard(["type": "reminder", "title": "Tea", "kind": "reminder"])!]
        let question = try XCTUnwrap(DaemonApproval([
            "approval_id": "ap-1", "capability": "ferb.confirm", "kind": "owner_question", "task_id": "t-1",
            "reason": "Go ahead?", "options": ["Yes", "No"], "summary": "Run this in Terminal?",
            "detail": "FERB types it and presses Return.", "exact": "ls -la", "why": "To list the folder.",
            "evidence": ["Terminal is frontmost"]]))
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults("ChatTextSelection"))
        let host = NSHostingView(rootView: ConversationTranscript(
            entries: [.turn(cardTurn, answeredBelow: false), .turn(answered, answeredBelow: false)],
            questions: [question], model: model))
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 440, height: 2400),
                              styleMask: .borderless, backing: .buffered, defer: true)
        window.contentView = host                                   // never ordered front or captured
        model.preferredTranscriptHeight = 2400
        await settle(host)
        let strings = textViews(in: host).map(\.string)
        for said in [failedText, "1 of 3 done", "Run this in Terminal?", "To list the folder.", "ls -la",
                     "my dentist is Dr. Rao"] {
            XCTAssertTrue(strings.contains { $0.contains(said) }, "\(said) is FERB's own text view")
        }
        XCTAssertTrue(strings.contains { $0.hasPrefix("Won't run") }, "the won't-run row is FERB's own text view")
        XCTAssertFalse(hasSwiftUISelectableText(in: host), "no SwiftUI selectable text in the chat")
        withExtendedLifetime(window) {}
    }

    // decision: APP-788
    func testACollapsedTextFadesAndClipsItselfAndTakesNoClicksBelow() throws {
        let view = MessageTextView()
        view.update(NativeMessageText((1...20).map { "Line \($0) of the text" }.joined(separator: "\n")).text,
                    generation: 0)
        let size = view.measuredSize(width: 300)
        view.frame = CGRect(origin: .zero, size: size)
        view.layout()
        func alpha(below shown: CGFloat?) throws -> (top: CGFloat, fade: CGFloat, hidden: CGFloat) {
            view.shownHeight = shown
            let rep = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
            view.cacheDisplay(in: view.bounds, to: rep)
            func band(_ from: CGFloat, _ to: CGFloat) -> CGFloat {
                var most: CGFloat = 0
                let scale = CGFloat(rep.pixelsHigh) / view.bounds.height
                for y in stride(from: Int(from * scale), to: Int(to * scale), by: 1) {
                    for x in stride(from: 0, to: rep.pixelsWide, by: 2) {
                        most = max(most, rep.colorAt(x: x, y: y)?.alphaComponent ?? 0)
                    }
                }
                return most
            }
            return (band(0, 40), band(88, 98), band(110, size.height))
        }
        let open = try alpha(below: nil)
        XCTAssertGreaterThan(open.hidden, 0.5, "open: all of it draws")
        let collapsed = try alpha(below: 100)
        XCTAssertGreaterThan(collapsed.top, 0.5, "collapsed: its first lines draw")
        XCTAssertLessThan(collapsed.fade, collapsed.top, "its last shown lines fade")
        XCTAssertEqual(collapsed.hidden, 0, "nothing below the shown height draws")
        let container = NSView(frame: CGRect(origin: .zero, size: size))
        container.addSubview(view)
        XCTAssertNil(view.hitTest(CGPoint(x: 10, y: container.isFlipped ? 150 : size.height - 150)),
                     "the hidden part takes no clicks")
        view.shownHeight = nil
        XCTAssertNotNil(view.hitTest(CGPoint(x: 10, y: container.isFlipped ? 150 : size.height - 150)))
    }

    private func part(_ part: String, state: String, text: String, model: NotchModel) {
        model.receiveBusEvent(["topic": "task.part", "data": ["type": "part", "task_id": "T", "request_id": "R",
                                                              "part": part, "state": state, "text": text,
                                                              "seq": "2026-11-07T10:0\(state.count % 10):00+00:00"]])
    }

    private func textViews(in view: NSView) -> [MessageTextView] {
        if let field = view as? MessageTextView { return [field] }
        return view.subviews.flatMap { textViews(in: $0) }
    }

    /// SwiftUI draws a selectable `Text` as an AppKitTextInteractionView whose glyphs sit in its own sublayer.
    private func hasSwiftUISelectableText(in view: NSView) -> Bool {
        if String(describing: type(of: view)).contains("TextInteraction") { return true }
        return view.subviews.contains { hasSwiftUISelectableText(in: $0) }
    }

    private func settle(_ host: NSView) async {
        for _ in 0..<3 {
            host.layoutSubtreeIfNeeded()
            await withCheckedContinuation { continuation in DispatchQueue.main.async { continuation.resume() } }
        }
        host.layoutSubtreeIfNeeded()
    }
}

/// The transcript as the notch shows it: today's entries, the reveal's generation in the environment (NotchView).
private struct TranscriptHost: View {
    @ObservedObject var model: NotchModel
    var body: some View {
        ConversationTranscript(entries: model.visibleEntries, questions: model.inlineQuestions, model: model)
            .environment(\.selectableTextGeneration, model.textRenderGeneration)
    }
}
