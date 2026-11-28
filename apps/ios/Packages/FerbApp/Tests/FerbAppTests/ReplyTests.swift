#if canImport(UIKit)
import FerbAppModel
import FerbCore
import FerbDesign
import FerbMobile
import SwiftUI
import XCTest
@testable import FerbAppUI

/// Reply to a message (the owner, 2026-11-27, kb APP-1146, APP-1147; FERB.app's ReplyQuotes.swift): which bubbles the
/// phone library offers a quote for and what his message quotes (through the real library on the simulator), the
/// quote in the message box and with the message sent, a tap on a quote, and goldens of the box's quote and of a sent
/// message with its quote (Screenshots/app/parity/reply-*).
@MainActor
final class ReplyTests: XCTestCase {
    private var pos = 10
    private var rig: Rig!

    override func setUp() async throws { rig = Rig() }
    override func tearDown() async throws { rig.tearDown() }

    private func entry(_ id: String, _ kind: String, _ request: String?, _ body: [String: Any], task: String? = nil,
                       answers: String? = nil, ref: String? = nil) -> [String: Any] {
        pos += 1
        return ["id": id, "conversation_id": "C1", "pos": pos, "seq": pos, "rev": 1, "kind": kind, "state": "done", "open": false,
                "request_id": request ?? NSNull(), "task_id": task ?? NSNull(), "approval_id": NSNull(), "ref": ref ?? NSNull(),
                "answers": answers ?? NSNull(), "placement": kind == "answer" ? "inline" : NSNull(), "origin": NSNull(), "client_id": "phone-1",
                "submission_id": NSNull(), "journal_id": NSNull(), "created_at": "2026-01-02T09:00:00+00:00", "updated_at": "2026-01-02T09:00:00+00:00",
                "day": "2026-01-02", "body": body, "deleted": false]
    }

    private func message(_ id: String, _ request: String, _ text: String, task: String, replyTo: [String: Any]? = nil) -> [String: Any] {
        var body: [String: Any] = ["text": text, "attachments": [String](), "hidden": [String]()]
        if let replyTo { body["reply_to"] = replyTo }
        return entry(id, "message", request, body, task: task)
    }

    private func answer(_ id: String, _ request: String, _ of: String, _ text: String, task: String) -> [String: Any] {
        entry(id, "answer", request, ["from": "task", "text": text, "status": "done", "verified": true, "artifacts": [String](),
                                      "note": NSNull(), "lines": [String](), "cards": [[String: Any]]()], task: task, answers: of)
    }

    /// His table booked, then his reply to FERB's answer ("Make it 8 instead", quoting it), and FERB's note.
    private func chat() throws -> TranscriptView {
        try FixtureMac.transcript(FixtureMac(FixtureConversation(file: "invented", scenario: "reply", conversation: "C1", entries: [
            message("m1", "r1", "Book a table for two at Nori tonight", task: "t-1"),
            entry("p1", "plan", "r1", ["text": "I’ll look for a table for two at Nori tonight.", "plan_line": "I’ll look for a table for two at Nori tonight.",
                                       "lines": [String](), "name": "Book a table"], task: "t-1"),
            answer("a1", "r1", "m1", "Booked the **7pm** table at [Nori](https://nori.example). They hold it for 15 minutes.", task: "t-1"),
            message("m2", "r2", "Make it 8 instead", task: "t-2",
                    replyTo: ["entry_id": "a1", "kind": "answer", "who": "ferb", "excerpt": "Booked the **7pm** table at Nori. They hold it for 15 minutes."]),
            answer("a2", "r2", "m2", "Moved it to 8pm. Same table.", task: "t-2"),
            entry("n1", "notice", nil, ["text": "Your Mac's disk is nearly full.", "quotes": [Any]()], ref: "disk-n1"),
        ])))
    }

    private let booked = ReplyQuote(entryId: "a1", kind: "answer", byOwner: false, excerpt: "Booked the 7pm table at Nori. They hold it for 15 minutes.")

    // decision: APP-1146
    func testEachBubbleOffersItsOwnQuoteAndHisReplyShowsWhatItQuotes() throws {
        let view = try chat()
        let rows = ChatModel.rows(view)
        let replies = ChatModel.replies(view)
        // his own message, FERB's first reply, its answer and its note each offer theirs (APP-1147: "my messages too")
        XCTAssertEqual(replies["m1"]?.quote, ReplyQuote(entryId: "m1", kind: "message", byOwner: true, excerpt: "Book a table for two at Nori tonight"))
        XCTAssertEqual(replies["m1:first"]?.quote?.entryId, "p1")
        XCTAssertEqual(replies["m1:first"]?.quote?.kind, "plan")
        let answerKey = try XCTUnwrap(ChatModel.quotedRow(rows, replies, entryId: "a1"))
        guard case .answer = try XCTUnwrap(rows.first { $0.key == answerKey }) else { return XCTFail("a1 is the answer's bubble: \(answerKey)") }
        XCTAssertEqual(replies[answerKey]?.quote, booked, "one line, without Markdown's marks")
        XCTAssertNotNil(ChatModel.quotedRow(rows, replies, entryId: "n1"), "a note FERB posted")
        // what his reply quotes, as the engine kept it, read without its marks
        XCTAssertEqual(replies["m2"]?.quoting, booked)
        XCTAssertNil(replies["m1"]?.quoting)
        // every row that offers a quote is one the chat draws
        for key in replies.keys where replies[key]?.quote != nil && !key.hasSuffix(":card:answer") {
            XCTAssertTrue(rows.contains { $0.key == key } || key.hasSuffix(":answer"), "\(key) is a row")
        }
    }

    private func opened() async throws -> AppModel {
        rig.phone.view = try chat()
        let vm = rig.model()
        rig.session.unlocked()
        await vm.refresh()?.value
        await eventually("the chat") { vm.rows.contains { $0.key == "m2" } && !vm.replies.isEmpty }
        return vm
    }

    // decision: APP-1146
    func testAReplyWaitsInTheBoxTakesTheCursorAndGoesWithTheMessage() async throws {
        let vm = try await opened()
        vm.reply(to: booked)
        XCTAssertEqual(vm.shownQuote, booked)
        XCTAssertTrue(vm.focusAsk, "the box takes the cursor")
        vm.typed("Make it 9 then")
        await vm.send().value
        XCTAssertEqual(rig.phone.sentWith.first?.0, "Make it 9 then")
        XCTAssertEqual(rig.phone.sentQuotes, [booked], "sent as its reply_to")
        XCTAssertNil(vm.shownQuote, "it went with the words")
        XCTAssertEqual(vm.draft, "")
        // the message waiting in the outbox shows what it quotes
        guard case .outbox(_, let item)? = vm.rows.last else { return XCTFail("the outbox row: \(vm.rows.map(\.key))") }
        XCTAssertEqual(item.replyTo, booked)
        // without a quote a message goes as before
        vm.typed("Thanks")
        await vm.send().value
        XCTAssertTrue(rig.phone.calls.contains("send:Thanks"))
        XCTAssertEqual(rig.phone.sentQuotes.count, 1)
    }

    // decision: APP-1146
    func testAFailedSendKeepsItsWordsAndItsQuoteAndTheCrossDropsOnlyTheQuote() async throws {
        let vm = try await opened()
        rig.phone.sendWithResult = .failed(.Refused(detail: "The Mac said no.", linkCode: nil))
        vm.reply(to: booked)
        vm.typed("Make it 9 then")
        await vm.send().value
        XCTAssertEqual(vm.draft, "Make it 9 then")
        XCTAssertEqual(vm.shownQuote, booked, "it comes back with the words")
        vm.dropReplyQuote()
        XCTAssertNil(vm.shownQuote)
        XCTAssertEqual(vm.draft, "Make it 9 then", "the words stay")
    }

    // decision: APP-1146, ORCH-715
    func testAQuoteShowsOnlyInTheChatItWasTakenIn() async throws {
        let vm = try await opened()
        vm.reply(to: booked)
        await vm.toggleTemporary().value
        XCTAssertNotNil(vm.temporary)
        XCTAssertNil(vm.shownQuote, "not in the temporary chat")
        vm.typed("What is 2+2?")
        await vm.send().value
        XCTAssertTrue(rig.phone.sentQuotes.isEmpty, "the temporary chat's message quotes nothing")
        await vm.toggleTemporary().value
        XCTAssertEqual(vm.shownQuote, booked, "back in the chat it waits still")
    }

    // decision: APP-1146
    func testATapOnAQuoteBringsTheQuotedMessageIntoViewLitUpAndNothingMovesWhenThePhoneHasntIt() async throws {
        let vm = try await opened()
        // the invented chat is of an earlier day: from today's chat the earlier days open with it in view
        XCTAssertFalse(vm.earlier)
        vm.showQuoted(booked)
        let key = try XCTUnwrap(ChatModel.quotedRow(vm.rows, vm.replies, entryId: "a1"))
        XCTAssertTrue(vm.earlier, "the earlier days, where it is")
        XCTAssertEqual(vm.chatReveal?.id, key)
        XCTAssertEqual(vm.quoteLit, key)
        await eventually("the light goes", timeout: 3) { vm.quoteLit == nil }
        // a quote of a message older than what this phone holds: nothing moves (FERB.app, Android)
        let n = vm.chatReveal?.n
        vm.showQuoted(ReplyQuote(entryId: "older", kind: "answer", byOwner: false, excerpt: "Long ago"))
        XCTAssertEqual(vm.chatReveal?.n, n)
        XCTAssertTrue(vm.earlier)
        XCTAssertNil(vm.quoteLit)
        // a question's box asked for later wins
        vm.goToQuestion("ap-1")
        XCTAssertEqual(vm.chatReveal?.id, "q:ap-1")
    }

    // MARK: goldens (Screenshots/app/parity)

    private func shot<V: View>(_ v: V, _ name: String, height: CGFloat, hour: Double) {
        let page = PhoneScreen(moment: .lab(hour), glide: false) {
            v.padding(.horizontal, 16).padding(.top, 24).frame(maxHeight: .infinity, alignment: .top)
        }
        .frame(width: 402, height: height)
        .environment(\.chatViewport, 700)
        .stillMotion()
        AppSnapshot.assert(page, size: CGSize(width: 402, height: height), path: "app/parity/\(name).png")
        Self.keep(page, name, height: height)
    }

    /// A copy for the owner's look (FERB_REPLY_SHOTS names the folder; nothing is written without it).
    private static func keep<V: View>(_ page: V, _ name: String, height: CGFloat) {
        guard let dir = ProcessInfo.processInfo.environment["FERB_REPLY_SHOTS"], !dir.isEmpty else { return }
        let url = URL(fileURLWithPath: dir).appendingPathComponent("\(name).png")
        try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        let image = AppSnapshot.render(page, size: CGSize(width: 402, height: height))
        try? image.pngData()?.write(to: url)
    }

    /// The quote waiting in the message box, his and FERB's, as the box draws it above his words.
    // decision: APP-1146
    func testGoldenTheQuoteInTheMessageBox() throws {
        let m = rig.model()
        let mine = ReplyQuote(entryId: "m1", kind: "message", byOwner: true, excerpt: "Book a table for two at Nori tonight")
        let v = VStack(spacing: 16) {
            PhoneComposer(typed: "Make it 8 instead", attachments: AnyView(ReplyQuoteBar(quoteShown(booked, model: m), dropLabel: "x") {}))
            PhoneComposer(typed: "", attachments: AnyView(ReplyQuoteBar(quoteShown(mine, model: m), dropLabel: "x") {}))
        }
        for (hour, name) in [(12.5, "noon"), (22.5, "night")] { shot(v, "reply-composer-\(name)", height: 300, hour: hour) }
    }

    /// FERB's answer, his reply under it with what it quotes, one more reply waiting in the outbox, and the answer lit
    /// up as a tap on the quote shows it.
    // decision: APP-1146
    func testGoldenASentMessageWithItsQuote() throws {
        let view = try chat()
        let rows = ChatModel.rows(view)
        let replies = ChatModel.replies(view)
        let m = rig.model()
        let answerKey = try XCTUnwrap(ChatModel.quotedRow(rows, replies, entryId: "a1"))
        let shown = rows.filter { $0.key == answerKey || $0.key == "m2" }
        let waiting = OutboxItem(id: "o1", text: "And a cake for dessert", createdAt: "2026-01-02T10:00:00+00:00", state: "waiting",
                                 waitingFor: "link", reason: nil, tried: false, attachments: [],
                                 replyTo: ReplyQuote(entryId: "m2", kind: "message", byOwner: true, excerpt: "Make it 8 instead"))
        func page(lit: Bool) -> some View {
            VStack(spacing: 12) {
                ForEach(shown) { ChatRowView(model: m, row: $0, today: 0, olderLoading: false, reply: replies[$0.key], lit: lit && $0.key == answerKey) }
                ChatRowView(model: m, row: .outbox(key: "o:o1", item: waiting), today: 0, olderLoading: false)
            }
        }
        for (hour, name) in [(12.5, "noon"), (22.5, "night")] { shot(page(lit: false), "reply-sent-\(name)", height: 520, hour: hour) }
        shot(page(lit: true), "reply-sent-lit-night", height: 520, hour: 22.5)
    }

    /// A swipe under way: FERB's answer past the reach (the arrow dark, letting go replies), his own message part way.
    // decision: APP-1146
    func testGoldenASwipeUnderWay() throws {
        let view = try chat()
        let rows = ChatModel.rows(view)
        let replies = ChatModel.replies(view)
        let m = rig.model()
        let answerKey = try XCTUnwrap(ChatModel.quotedRow(rows, replies, entryId: "a1"))
        let answer = try XCTUnwrap(rows.first { $0.key == answerKey })
        let mine = try XCTUnwrap(rows.first { $0.key == "m1" })
        let v = VStack(spacing: 12) {
            ChatRowView(model: m, row: mine, today: 0, olderLoading: false, reply: replies["m1"]).environment(\.replyPullForShot, 40)
            ChatRowView(model: m, row: answer, today: 0, olderLoading: false, reply: replies[answerKey]).environment(\.replyPullForShot, 80)
        }
        for (hour, name) in [(12.5, "noon"), (22.5, "night")] { shot(v, "reply-swipe-\(name)", height: 360, hour: hour) }
    }
}
#endif
