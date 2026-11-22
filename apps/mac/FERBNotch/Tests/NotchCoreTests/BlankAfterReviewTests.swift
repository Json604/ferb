import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Owner 2026-11-23, after answering the import review with Approve all: "its blank after i approved all the memory
/// messages" (the notch open with only its header; no messages, no greeting, no composer; the same after a relaunch).
/// His state, with invented words: today held only notices, said after a turn of an earlier day that the app hadn't read
/// since it opened, most of them folded into the review (MEM-461); a website import's live card waited for its mail
/// (MEM-456); and the review's question went once its list was answered. Each piece is driven through the real model
/// and the notch's real views.
@MainActor
final class BlankAfterReviewTests: XCTestCase {
    private let today = Calendar.current.startOfDay(for: Date())

    /// A notice as FERB.app saves it in FERB's database (`chat.history.*`), said after `after` (a turn the app may not
    /// hold); `folded`: what the daemon marks beside it on the page.
    private func notice(_ n: Int, ref: String, text: String, after: UUID?, folded: String? = nil) -> [String: Any] {
        let id = UUID()
        let at = today.addingTimeInterval(TimeInterval(60 * n + 30))
        var body: [String: Any] = ["id": ref, "entryID": id.uuidString, "text": text, "quotes": [],
                                   "createdAt": at.timeIntervalSinceReferenceDate]
        if let after { body["afterTurnID"] = after.uuidString }
        var row: [String: Any] = ["id": id.uuidString, "kind": "notice", "day": HistoryEntry.day(at), "open": false,
                                  "at": Date.ISO8601FormatStyle(includingFractionalSeconds: true).format(at),
                                  "ref": ref, "body": body]
        if let folded { row["folded"] = folded }
        return row
    }

    private func daemonModel(_ rpc: HistoryRPC, _ name: String) throws -> NotchModel {
        let defaults = scratchDefaults(name)
        defaults.set(try JSONEncoder().encode(HistoryImport(at: Date(), entries: 0, archiveSHA256: "")),
                     forKey: NotchModel.historyImportKey)
        let model = NotchModel(rpc: rpc, preferences: defaults)
        XCTAssertEqual(model.chatStore.store, .daemon)
        return model
    }

    private static let waitingCard = MemoryImportCard(title: "Bringing in what ChatGPT knows about you", rows: [
        .init(key: "site:chatgpt", source: "chatgpt", name: "ChatGPT", stage: "waiting_mail",
              line: "Waiting for ChatGPT's email — asked at 00:52",
              detail: "I look in your email for it every 12 hours. It can take a day or two."),
    ])

    private static func reviewQuestion() throws -> DaemonApproval {
        try XCTUnwrap(DaemonApproval([
            "approval_id": "ap-review", "kind": "owner_question", "capability": "memory.review",
            "reason": "Can you check a few things I brought in?\n6 things to keep in mind all the time. Answer each one here, or just tell me.",
            "summary": "Can you check a few things I brought in?",
            "detail": "6 things to keep in mind all the time. Answer each one here, or just tell me.",
        ]))
    }

    /// The notch as it opens over the model, laid out in a window that is never ordered front: where the composer's
    /// text view stands, top-down from the surface's top, and the body's own band.
    private func composerPlace(_ model: NotchModel) throws -> (composer: CGRect, body: ClosedRange<CGFloat>) {
        model.expand(focus: false)
        let presentation = NotchPresentation()
        let host = NSHostingView(rootView: NotchView(model: model, presentation: presentation))
        host.sizingOptions = []
        host.frame = CGRect(origin: .zero, size: model.panelSize)
        let window = NSWindow(contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: true)
        window.contentView = host
        presentation.present(size: model.surfaceSize, expanded: true, animate: false) {}
        for _ in 0..<4 { host.layoutSubtreeIfNeeded(); RunLoop.main.run(until: Date().addingTimeInterval(0.03)) }
        let text = try XCTUnwrap(Self.find(ChatTextView.self, in: host), "the composer is built")
        var frame = text.convert(text.bounds, to: host)
        if !host.isFlipped { frame.origin.y = host.bounds.height - frame.maxY }
        withExtendedLifetime(window) {}
        return (frame, model.cutoutHeight...(model.cutoutHeight + model.bodyHeight))
    }

    private static func find<T: NSView>(_ type: T.Type, in view: NSView) -> T? {
        if let hit = view as? T { return hit }
        return view.subviews.lazy.compactMap { find(type, in: $0) }.first
    }

    // decision: APP-1114
    // A reply or notice said after a turn the app doesn't hold (an earlier day's) still shows, in arrival order, before
    // the turns it holds: it came before them. It used to be left out, so a day of notices drew nothing.
    func testEntriesSaidAfterATurnNotInHandStillShow() throws {
        let earlier = UUID()
        let stored = try [notice(1, ref: "memory-import-r1", text: "I read what you've typed into Codex.", after: earlier),
                          notice(2, ref: "memory-offers-answered", text: "Got it, I've put those aside.", after: earlier)]
            .map { try XCTUnwrap(HistoryEntry.read($0)) }
        var archive = ConversationArchive()
        _ = archive.absorb(stored)
        XCTAssertEqual(archive.transcript(on: today).map(\.id), stored.map(\.id))
        // a turn made later that day follows them
        let turn = UUID()
        archive.begin(id: turn, request: "What's on my calendar?", sources: [], date: today.addingTimeInterval(600))
        XCTAssertEqual(archive.transcript(on: today).map(\.id), stored.map(\.id) + [turn])
    }

    // decision: MEM-461 — a notice the daemon marks folded on the page is kept and never drawn; so is one the engine
    // folds while the app runs
    func testFoldedNoticesAreKeptAndNotDrawn() async throws {
        let rpc = HistoryRPC()
        let earlier = UUID()
        _ = try await rpc.call("chat.history.import", params: ["entries": [
            notice(1, ref: "memory-always-on-m1", text: "Now always on: “The owner prefers tea.”", after: earlier,
                   folded: "memory-review"),
            notice(2, ref: "memory-always-on-m2", text: "Now always on: “The owner uses Arc.”", after: earlier),
            notice(3, ref: "memory-import-r1", text: "I read what you've typed into Codex.", after: earlier),
        ]])
        let model = try daemonModel(rpc, "BlankFolded")
        await model.loadDay(today)
        XCTAssertEqual(model.conversation.notices.count, 3, "all kept")
        XCTAssertEqual(model.visibleEntries.count, 2)
        let live = try XCTUnwrap(ChatEntry(["id": "e2", "kind": "notice", "pos": 2, "rev": 2, "state": "done",
                                            "ref": "memory-always-on-m2", "body": ["text": "Now always on",
                                                                                   "folded": "memory-review"]]))
        let applied = await model.applyEntryChange(live, live: true)
        XCTAssertTrue(applied)
        XCTAssertEqual(model.visibleEntries.count, 1)
        XCTAssertEqual(model.conversation.notices.count, 3, "marked, never deleted")
    }

    // decision: APP-1114
    // The import's live card alone is drawn as the transcript, so the chat takes the chat's height: sized as an empty
    // day it pushed the chat's rows and the composer out of the notch.
    func testTheLiveCardAloneKeepsTheComposerInTheNotch() throws {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults("BlankCardOnly"))
        model.memoryImport = Self.waitingCard
        XCTAssertTrue(model.visibleEntries.isEmpty && model.inlineQuestions.isEmpty)
        XCTAssertTrue(model.showsTranscript)
        XCTAssertEqual(model.bodyHeight, model.chatTranscriptHeight + model.conversationChromeHeight, accuracy: 0.5)
        let (composer, body) = try composerPlace(model)
        XCTAssertTrue(body.contains(composer.minY) && body.contains(composer.maxY),
                      "the composer \(composer) inside the body \(body)")
    }

    // decision: APP-1114, MEM-460
    // His sequence: the review answered with Approve all until nothing is left. Its card goes, today's notices (said
    // after an earlier day's turn, most folded into the review) and the live card show, at the chat's height, with the
    // composer in view.
    func testApproveAllUntilNothingIsLeftLeavesAChatWithItsComposer() async throws {
        let rpc = HistoryRPC()
        let earlier = UUID()
        var rows = (1...6).map {
            notice($0, ref: "memory-always-on-m\($0)", text: "Now always on: “Fact \($0).”", after: earlier,
                   folded: "memory-review")
        }
        rows.append(notice(7, ref: "memory-import-r1", text: "I read what you've typed into Codex.", after: earlier))
        rows.append(notice(8, ref: "memory-offers-answered", text: "Got it, I've put those aside.", after: earlier))
        _ = try await rpc.call("chat.history.import", params: ["entries": rows])
        let model = try daemonModel(rpc, "BlankApproveAll")
        await model.loadDay(today)
        model.memoryImport = Self.waitingCard
        model.pendingApprovals = [try Self.reviewQuestion()]
        XCTAssertEqual(model.inlineQuestions.count, 1)

        // Approve all: the daemon's reply is the list with nothing left (HistoryRPC answers it with no parts)
        let part = try XCTUnwrap(MemoryReview(MemoryReviewTests.sample).parts.first)
        await model.memoryReview.answer(part, ids: part.ids, with: part.all[0], all: true)
        XCTAssertEqual(model.memoryReview.review, .empty)
        XCTAssertTrue(model.pendingApprovals.isEmpty, "the review's card goes with its list")

        XCTAssertEqual(model.visibleEntries.count, 2, "today's notices, the folded ones left out")
        XCTAssertTrue(model.showsTranscript)
        XCTAssertEqual(model.bodyHeight, model.chatTranscriptHeight + model.conversationChromeHeight, accuracy: 0.5)
        let (composer, body) = try composerPlace(model)
        XCTAssertTrue(body.contains(composer.minY) && body.contains(composer.maxY),
                      "the composer \(composer) inside the body \(body)")
    }

    // A day with nothing to show at all is the empty day: its greeting and composer, at the empty day's height.
    func testAnEmptyDayShowsTheGreetingAndComposer() throws {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults("BlankEmpty"))
        XCTAssertFalse(model.showsTranscript)
        XCTAssertEqual(model.bodyHeight, min(31, model.chatTranscriptHeight) + model.conversationChromeHeight, accuracy: 0.5)
        let (composer, body) = try composerPlace(model)
        XCTAssertTrue(body.contains(composer.minY) && body.contains(composer.maxY),
                      "the composer \(composer) inside the body \(body)")
    }
}
