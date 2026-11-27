import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Reply to a message (owner 2026-11-27: "gotta [add] quote any ferb's reply message"; "yeah my messages too"): a
/// message whose engine entry id the app knows can be quoted; the quote waits above the composer, goes with the sent
/// message as `reply_to` and clears with the draft; Esc drops it; the sent message and restored history show it as the
/// engine kept it. Also Show more (owner 2026-11-27: "dont move me just expand the text").
@MainActor
final class ReplyQuoteTests: XCTestCase {
    // MARK: replying

    // decision: APP-1146
    func testASentReplyCarriesTheQuotedEntryAndTheComposerClearsIt() async throws {
        let rpc = EntriesRPC()
        rpc.script("chat.send", .frames([
            ["type": "ack", "request_id": "r1", "seq": 1, "entry_id": "m1", "message": "which iPhone?"],
            ["type": "reply", "request_id": "r1", "text": "Go for an **iPhone 7**, ideally 128 GB.", "status": "done"],
        ]), .frames([
            ["type": "ack", "request_id": "r2", "seq": 3, "entry_id": "m2", "message": "what about the 7 Plus?"],
        ]))
        let model = connected(rpc)
        await until { model.entryWatch.caughtUp }
        model.send("which iPhone?")
        await until { model.conversation.turns.first?.answer.isEmpty == false }
        let first = try XCTUnwrap(model.conversation.turns.first)
        XCTAssertEqual(first.entryID, "m1", "the ack names the message's entry")
        XCTAssertNil(model.quote(entryID: first.answerEntryID, kind: "answer", owner: false, text: first.answer),
                     "an answer whose entry isn't known yet offers no reply")
        // the engine's answer entry for this app's own turn: only its id is kept
        rpc.push("chat.entries.watch", change(entry("a1", pos: 2, seq: 2, kind: "answer", request: "r1",
                                                    client: model.clientID, body: ["text": first.answer, "status": "done"])))
        await until { model.conversation.turns.first?.answerEntryID == "a1" }

        let answered = try XCTUnwrap(model.conversation.turns.first)
        let quote = try XCTUnwrap(model.quote(entryID: answered.answerEntryID, kind: "answer", owner: false,
                                              text: answered.visibleAnswer))
        XCTAssertFalse(quote.byOwner)
        XCTAssertEqual(quote.line, "Go for an iPhone 7, ideally 128 GB.")
        model.reply(to: quote)
        XCTAssertEqual(model.shownComposerQuote, quote)
        XCTAssertGreaterThan(model.replyQuoteHeight, 0, "the quote line takes its room above the composer")
        model.session.draft = "what about the 7 Plus?"
        model.startWork()
        await until { rpc.params(of: "chat.send").count == 2 }
        XCTAssertEqual(rpc.params(of: "chat.send")[1]["reply_to"] as? String, "a1")
        XCTAssertNil(rpc.params(of: "chat.send")[0]["reply_to"], "a message that quotes nothing sends none")
        XCTAssertNil(model.shownComposerQuote, "the quote goes with the draft")
        XCTAssertEqual(model.session.draft, "")
        XCTAssertEqual(model.replyQuoteHeight, 0)
        let sent = try XCTUnwrap(model.conversation.turns.last)
        XCTAssertEqual(sent.replyTo?.entryID, "a1", "the sent bubble shows the quote at once")

        // the engine's own copy of the quote (its excerpt, redacted as it was shown) replaces the one made here
        await until { model.conversation.turns.last?.requestID == "r2" }
        rpc.push("chat.entries.watch", change(entry("m2", pos: 3, seq: 3, kind: "message", request: "r2",
                                                    client: model.clientID, body: [
                                                        "text": "what about the 7 Plus?",
                                                        "reply_to": ["entry_id": "a1", "kind": "answer", "who": "ferb",
                                                                     "excerpt": "Go for an iPhone 7, ideally 128 GB."]])))
        await until { model.conversation.turns.last?.replyTo?.kind == "answer" && model.conversation.turns.last?.entryID == "m2" }
        XCTAssertEqual(model.conversation.turns.last?.replyTo?.excerpt, "Go for an iPhone 7, ideally 128 GB.")
    }

    func testEscDropsTheQuoteAndKeepsTheDraft() {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults())
        model.session.draft = "and the case?"
        model.reply(to: ReplyQuote.of(entryID: "m1", kind: "message", owner: true, text: "which iPhone?"))
        XCTAssertEqual(model.shownComposerQuote?.speaker, "You")
        XCTAssertTrue(model.dropReplyQuote(), "Esc takes the quote first")
        XCTAssertNil(model.shownComposerQuote)
        XCTAssertEqual(model.session.draft, "and the case?", "the draft stays")
        XCTAssertFalse(model.dropReplyQuote(), "with no quote, Esc does what it did")
    }

    func testAQuoteStaysWithTheChatItWasTakenIn() {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults())
        model.reply(to: ReplyQuote.of(entryID: "a1", kind: "answer", owner: false, text: "Done."))
        model.composerQuote = ComposerQuote(quote: model.composerQuote!.quote, source: .temporary)
        XCTAssertNil(model.shownComposerQuote, "a quote of another chat never shows or goes with this chat's message")
        XCTAssertNil(model.takeComposerQuote())
    }

    // MARK: restored history

    // decision: APP-1146
    func testRestoredHistoryShowsTheQuote() throws {
        // this app's own turn, as the chat history keeps it
        var turn = ConversationTurn(id: UUID(), createdAt: Date(), request: "what about the 7 Plus?", sources: [])
        turn.status = "done"
        turn.entryID = "m2"
        turn.replyTo = ReplyQuote(entryID: "a1", kind: "answer", who: "ferb", excerpt: "Go for an iPhone 7")
        let wire = try HistoryEntry(.turn(turn)).wire
        guard case let .turn(restored)? = HistoryEntry.read(wire) else { return XCTFail("the turn reads back") }
        XCTAssertEqual(restored.replyTo, turn.replyTo)
        XCTAssertEqual(restored.entryID, "m2")

        // another device's turn, from the engine's entries: the message's body carries the quote
        let entries = ChatEntry.list([
            entry("m1", pos: 1, seq: 1, kind: "message", request: "r1", client: "phone", body: ["text": "which iPhone?"]),
            entry("a1", pos: 2, seq: 2, kind: "answer", request: "r1", client: "phone",
                  body: ["text": "Go for an iPhone 7.", "status": "done"], answers: "m1"),
            entry("m2", pos: 3, seq: 3, kind: "message", request: "r2", client: "phone", body: [
                "text": "what about the 7 Plus?",
                "reply_to": ["entry_id": "a1", "kind": "answer", "who": "ferb", "excerpt": "Go for an iPhone 7."]]),
        ])
        let archive = EntryFold.archive(entries)
        XCTAssertEqual(archive.turns.map(\.entryID), ["m1", "m2"])
        XCTAssertEqual(archive.turns[0].answerEntryID, "a1")
        XCTAssertEqual(archive.turns[1].replyTo?.entryID, "a1")
        XCTAssertEqual(archive.turns[1].replyTo?.speaker, "FERB")
        XCTAssertNil(archive.turns[0].replyTo)
        // a quote's click finds what it quotes: the first turn's answer, and the owner's message by its turn
        XCTAssertEqual(archive.shownTarget(ofEntry: "a1"), TranscriptTarget.answer(turn: archive.turns[0].id))
        XCTAssertEqual(archive.shownTarget(ofEntry: "m1"), TranscriptTarget.entry(archive.turns[0].id))
        XCTAssertNil(archive.shownTarget(ofEntry: "elsewhere"))
    }

    func testClickingAQuoteBringsTheQuotedMessageForwardAndLightsItUp() {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults())
        let id = UUID()
        model.conversation.begin(id: id, request: "which iPhone?", sources: [])
        model.conversation.finish(id: id, answer: "Go for an iPhone 7.", status: "done", artifacts: [])
        model.conversation.noteMessageEntry(id: id, entryID: "m1")
        model.showQuoted(ReplyQuote.of(entryID: "m1", kind: "message", owner: true, text: "which iPhone?"))
        XCTAssertEqual(model.scrollRequest?.target, TranscriptTarget.entry(id))
        XCTAssertEqual(model.scrollRequest?.reason, .asked)
        XCTAssertEqual(model.quoteHighlight, "m1")
        // the owner asked: out of view it comes, never a pill, even right after he scrolled
        var follow = TranscriptFollow()
        follow.observe(offset: 0, contentHeight: 2000, viewport: 300, at: Date())
        follow.observe(offset: 900, contentHeight: 2000, viewport: 300, at: Date())
        let request = TranscriptScrollRequest(target: "t", reason: .asked, generation: 1, revealing: false)
        XCTAssertEqual(follow.decide(request, target: -800 ... -700, viewport: 300, reduceMotion: false, at: Date()),
                       .scroll(target: "t", animated: true))
        XCTAssertEqual(follow.decide(request, target: 40 ... 90, viewport: 300, reduceMotion: false, at: Date()), .none)
    }

    // MARK: Show more

    // decision: APP-1147
    func testShowMoreRequestsNoScrollAndHoldsTheTextWhereItIs() {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults())
        var held: [String] = []
        model.textOpening = NotchModel.TextOpening(owner: ObjectIdentifier(self)) { held.append($0) }
        let key = ExpansionState.key(turn: UUID(), item: "answer"), target = "answer-x"
        model.toggleExpanded(key, target: target)
        XCTAssertTrue(model.expandedText.isOpen(key))
        XCTAssertEqual(held, [target], "the transcript holds the text's place before it grows")
        XCTAssertTrue(model.scrollRequests.isEmpty, "opening asks for no scroll")
        model.toggleExpanded(key, target: target)
        XCTAssertEqual(model.scrollRequests.last?.reason, .collapse, "Show less still keeps the top in view")
        XCTAssertEqual(held, [target])

        // held, growth below is never followed to the end
        var follow = TranscriptFollow()
        follow.observe(offset: 700, contentHeight: 1000, viewport: 300, at: Date())
        XCTAssertTrue(follow.following)
        follow.hold()
        XCTAssertFalse(follow.observe(offset: 700, contentHeight: 2400, viewport: 300, at: Date()))
    }

    // decision: APP-1147
    func testShowAllAtTheEndOfTheChatOpensDownwardsFromWhereTheTextIs() async throws {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults("ShowAllStays"))
        model.preferredTranscriptHeight = 400
        var short = ConversationTurn(id: UUID(), createdAt: Date(), request: "the fee?", sources: [])
        short.status = "done"
        short.answer = "60 euros."
        var long = ConversationTurn(id: UUID(), createdAt: Date().addingTimeInterval(5), request: "the visa rules", sources: [])
        long.status = "done"
        long.answer = (1...80).map { "- 7.\($0) a long bullet about the visa rules and what to bring" }.joined(separator: "\n")
        let host = NSHostingView(rootView: ConversationTranscript(
            entries: [.turn(short, answeredBelow: false), .turn(long, answeredBelow: false)], model: model))
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 440, height: 400),
                              styleMask: .borderless, backing: .buffered, defer: true)
        window.contentView = host                                   // never ordered front or captured
        func settle() async {
            for _ in 0..<6 {
                host.layoutSubtreeIfNeeded()
                await withCheckedContinuation { continuation in DispatchQueue.main.async { continuation.resume() } }
            }
            host.layoutSubtreeIfNeeded()
        }
        func answerTop() throws -> (onScreen: CGFloat, atEnd: Bool, folded: Bool) {
            let answer = try XCTUnwrap(textViews(in: host).first { $0.string.contains("7.1 a long") })
            let scroll = try XCTUnwrap(answer.enclosingScrollView)
            let list = try XCTUnwrap(scroll.documentView)
            let top = answer.convert(answer.bounds, to: list).minY - scroll.contentView.bounds.minY
            let atEnd = scroll.contentView.bounds.maxY >= list.frame.height - 2
            return (top, atEnd, answer.shownHeight != nil)
        }
        await settle()
        let before = try answerTop()
        XCTAssertTrue(before.folded, "the long answer is folded")
        XCTAssertTrue(before.atEnd, "the chat opens at its end")

        model.toggleExpanded(ExpansionState.key(turn: long.id, item: "answer"), target: TranscriptTarget.answer(turn: long.id))
        await settle()
        let after = try answerTop()
        XCTAssertFalse(after.folded, "Show all shows all of it")
        XCTAssertEqual(after.onScreen, before.onScreen, accuracy: 1, "the text's top stays where it was on screen")
        XCTAssertFalse(after.atEnd, "the view never jumps to the end of the opened text")
        withExtendedLifetime(window) {}
    }

    // MARK: helpers

    private func connected(_ rpc: EntriesRPC) -> NotchModel {
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults())
        model.entryWatch.retryWait = .milliseconds(10)
        model.entryWatch.catchUpWait = .milliseconds(500)
        model.connectDaemon()
        addTeardownBlock { @MainActor in model.disconnectDaemon(); rpc.endAll() }
        return model
    }

    private func until(_ done: () -> Bool, timeout: TimeInterval = 30, file: StaticString = #filePath,
                       line: UInt = #line) async {
        let end = Date().addingTimeInterval(timeout)
        while !done() {
            if Date() > end { XCTFail("timed out", file: file, line: line); return }
            try? await Task.sleep(for: .milliseconds(5))
        }
    }

    private func entry(_ id: String, pos: Int, seq: Int, kind: String, request: String, client: String,
                       body: [String: Any], answers: String? = nil) -> [String: Any] {
        let at = Date.ISO8601FormatStyle(includingFractionalSeconds: true).format(Date())
        var out: [String: Any] = ["id": id, "conversation_id": "S1", "pos": pos, "seq": seq, "rev": 1, "kind": kind,
                                  "state": "done", "open": false, "request_id": request, "client_id": client,
                                  "created_at": at, "updated_at": at, "day": HistoryEntry.day(Date()), "body": body,
                                  "deleted": false]
        if let answers { out["answers"] = answers }
        return out
    }

    private func change(_ entry: [String: Any]) -> [String: Any] { ["type": "change", "entry": entry] }

    private func textViews(in view: NSView) -> [MessageTextView] {
        if let field = view as? MessageTextView { return [field] }
        return view.subviews.flatMap { textViews(in: $0) }
    }
}
