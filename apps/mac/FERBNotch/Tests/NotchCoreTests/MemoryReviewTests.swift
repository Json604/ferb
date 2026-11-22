import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// FERB's review of what an import brought in (kb MEM-460..457) on the Mac: the daemon's list read as it comes, an
/// answer sent for one item or a whole part and the reply shown at once, the old "Now always on" notices folded away
/// (MEM-461), and a burst told as one notice whose cards open (MEM-463). The PNGs for the owner's review are drawn
/// only when MEMORY_REVIEW_SHOTS names a folder.
@MainActor
final class MemoryReviewTests: XCTestCase {
    static let sample: [String: Any] = [
        "count": 8,
        "parts": [
            ["part": "always_on", "title": "Keep these in mind all the time?",
             "hint": "I'll remember all of them either way. The ones you approve, I'll keep in mind in everything I do.",
             "groups": [
                ["title": "How you like me to work", "items": [
                    ["id": "i1", "text": "You prefer short, concise text in chat.", "note": "Already always on",
                     "answers": [["answer": "approve", "label": "Keep"], ["answer": "decline", "label": "Remove"]]],
                    ["id": "i2", "text": "You want to talk things through before coding starts.", "note": ""],
                    ["id": "i3", "text": "You like replies in bullet points.", "note": ""],
                    ["id": "i4", "text": "You want sources named when I answer from the web.", "note": ""],
                ]],
                ["title": "Your setup", "items": [
                    ["id": "i5", "text": "You test on a Motorola Edge 60 Fusion over wireless debugging.",
                     "note": "Already always on", "answers": [["answer": "approve", "label": "Keep"], ["answer": "decline", "label": "Remove"]]],
                    ["id": "i8", "text": "You use Arc as your main browser.", "note": "Already always on", "answers": [["answer": "approve", "label": "Keep"], ["answer": "decline", "label": "Remove"]]],
                ], "all": [["answer": "approve", "label": "Keep all"], ["answer": "decline", "label": "Remove all"]]],
             ],
             "answers": [["answer": "approve", "label": "Approve"], ["answer": "decline", "label": "Decline"]],
             "all": [["answer": "approve", "label": "Approve all"], ["answer": "decline", "label": "Decline all"]]],
            ["part": "still_true", "title": "Are these still true?", "hint": "You told me these a while ago.",
             "groups": [["title": "", "items": [
                ["id": "i6", "text": "You value sound feedback in apps.", "note": "Last said 20 Sep 2026"]]]],
             "answers": [["answer": "yes", "label": "Yes"], ["answer": "no", "label": "No, not any more"],
                         ["answer": "unsure", "label": "Not sure"]],
             "all": [["answer": "yes", "label": "Yes to all"], ["answer": "no", "label": "No to all"]]],
            ["part": "offers", "title": "Still want help with these?", "hint": "I won't start any of them on my own.",
             "groups": [["title": "", "items": [["id": "i7", "text": "Book the dentist for March", "note": ""]]]],
             "answers": [["answer": "open", "label": "Yes"], ["answer": "done", "label": "Done already"],
                         ["answer": "not interested", "label": "No thanks"]],
             "all": [["answer": "open", "label": "Yes to all"], ["answer": "not interested", "label": "No to all"]]],
        ],
    ]

    // decision: MEM-460
    func testTheReviewReadsAsTheDaemonSendsIt() {
        let review = MemoryReview(Self.sample)
        XCTAssertEqual(review.count, 8)
        XCTAssertEqual(review.parts.map(\.title),
                       ["Keep these in mind all the time?", "Are these still true?", "Still want help with these?"])
        XCTAssertEqual(review.parts[0].groups.map(\.title), ["How you like me to work", "Your setup"])
        XCTAssertEqual(review.parts[0].ids, ["i1", "i2", "i3", "i4", "i5", "i8"])
        // MEM-464: one already always on is kept or removed; a group all on has Keep all / Remove all
        let work = review.parts[0].groups[0], setup = review.parts[0].groups[1]
        XCTAssertEqual(work.items[0].buttons(in: review.parts[0]).map(\.label), ["Keep", "Remove"])
        XCTAssertEqual(work.items[1].buttons(in: review.parts[0]).map(\.label), ["Approve", "Decline"])
        XCTAssertEqual(setup.all.map(\.label), ["Keep all", "Remove all"])
        XCTAssertTrue(work.all.isEmpty)
        XCTAssertEqual(review.parts[1].answers.map(\.label), ["Yes", "No, not any more", "Not sure"])
        XCTAssertEqual(review.parts[0].groups[0].items[0].note, "Already always on")
        XCTAssertEqual(MemoryReview([:]), .empty)
    }

    // decision: MEM-460 — one item or a whole part, and the daemon's reply is the list at once
    func testAnAnswerSendsItsPartAndItemsAndShowsTheReply() async {
        let rpc = ScriptedRPC()
        rpc.replies["memory.review"] = Self.sample
        rpc.replies["memory.review.answer"] = ["count": 0, "parts": [[String: Any]]()]
        let model = MemoryReviewModel(rpc: rpc)
        await model.load()
        let part = model.review.parts[0]
        await model.answer(part, ids: ["i2"], with: part.answers[0])
        await model.answer(part, ids: part.ids, with: part.all[1], all: true)
        let sent = rpc.calls.filter { $0.0 == "memory.review.answer" }.map { $0.1 }
        XCTAssertEqual(sent.count, 2)
        XCTAssertEqual(sent[0]["part"] as? String, "always_on")
        XCTAssertEqual(sent[0]["item_ids"] as? [String], ["i2"])
        XCTAssertEqual(sent[0]["answer"] as? String, "approve")
        XCTAssertEqual(sent[1]["item_ids"] as? [String], ["i1", "i2", "i3", "i4", "i5", "i8"])
        XCTAssertEqual(sent[1]["answer"] as? String, "decline")
        XCTAssertEqual(model.review, .empty)
    }

    // decision: MEM-461 — a notice the engine folded into the review is kept in the store and never drawn
    func testNoticesFoldedIntoTheReviewAreNotDrawn() throws {
        func notice(_ id: String, _ pos: Int, _ body: [String: Any]) throws -> ChatEntry {
            try XCTUnwrap(ChatEntry(["id": id, "kind": "notice", "pos": pos, "rev": 2, "state": "done",
                                     "ref": "memory-always-on-\(id)", "created_at": "2026-11-23T00:16:00+05:30",
                                     "body": body]))
        }
        let chat = EntryFold.archive([
            try notice("a", 1, ["text": "Now always on: “The owner has the domain kartikey.xyz.” (FERB's understanding)",
                                "quotes": [], "folded": "memory-review"]),
            try notice("b", 2, ["text": "Now always on: 3 things (FERB's understanding)", "quotes": []]),
        ])
        XCTAssertEqual(chat.notices.map(\.text), ["Now always on: 3 things (FERB's understanding)"])
    }

    // MARK: pictures for the owner (not compared: the Mac has no pixel goldens)

    private func draw<V: View>(_ view: V, width: CGFloat, name: String) throws {
        guard let dir = ProcessInfo.processInfo.environment["MEMORY_REVIEW_SHOTS"], !dir.isEmpty else {
            throw XCTSkip("MEMORY_REVIEW_SHOTS")
        }
        let scene = view.frame(width: width).padding(16).background(Color.black).environment(\.colorScheme, .dark)
        let host = NSHostingView(rootView: scene)
        host.appearance = NSAppearance(named: .darkAqua)
        host.frame = CGRect(origin: .zero, size: CGSize(width: width + 32, height: 200))
        let window = NSWindow(contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: true)
        window.appearance = NSAppearance(named: .darkAqua)
        window.contentView = host
        for _ in 0..<4 { host.layoutSubtreeIfNeeded(); RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
        host.frame = CGRect(origin: .zero, size: host.fittingSize)
        window.setContentSize(host.fittingSize)
        for _ in 0..<4 { host.layoutSubtreeIfNeeded(); RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
        let rep = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: rep)
        try XCTUnwrap(rep.representation(using: .png, properties: [:]))
            .write(to: URL(fileURLWithPath: dir).appendingPathComponent(name))
    }

    func testDrawTheChatCardForLooking() async throws {
        let rpc = ScriptedRPC()
        rpc.replies["memory.review"] = Self.sample
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults("ferb-review-shots"))
        await model.memoryReview.load()
        let approval = try XCTUnwrap(DaemonApproval([
            "approval_id": "ap-review", "kind": "owner_question", "capability": "memory.review",
            "reason": "Can you check a few things I brought in?\n6 things to keep in mind all the time, 1 older thing "
                + "that may not be true any more and 1 thing I could help with. Answer each one here, or just tell me.",
            "summary": "Can you check a few things I brought in?",
            "detail": "6 things to keep in mind all the time, 1 older thing that may not be true any more and 1 thing "
                + "I could help with. Answer each one here, or just tell me.",
        ]))
        try draw(ApprovalView(model: model, approval: approval), width: 420, name: "mac-review-chat-card.png")
    }

    func testDrawTheMemoryPageForLooking() async throws {
        let rpc = ScriptedRPC()
        rpc.replies["memory.review"] = Self.sample
        rpc.replies["memory.list"] = ["items": [
            ["id": "m1", "kind": "preference", "content": "The owner prefers concise text in chat and schedule boxes.",
             "origin": "inferred", "label": "FERB's understanding", "about": "imported from Claude Code", "in_profile": true],
            ["id": "m2", "kind": "fact", "content": "The owner has the domain kartikey.xyz.", "origin": "inferred",
             "label": "FERB's understanding", "about": "imported from Claude Code", "in_profile": false],
        ]]
        rpc.replies["memory.profile"] = ["sections": [["title": "Preferences", "items": [["id": "m1"]]]],
                                         "used_tokens": 412, "budget_tokens": 2500]
        rpc.replies["memory.import.lines"] = ["lines": [[String: Any]]()]
        let store = FERBWindowStore(rpc: rpc)
        await store.loadMemory()
        await store.memoryReview.load()
        try draw(MemoryPage(store: store), width: 760, name: "mac-review-memory-page.png")
    }

    func testDrawAGroupedNoticeForLooking() throws {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults("ferb-review-notice"))
        let cards = ["The owner prefers tea.", "The owner runs every morning.", "The owner uses Arc for work."]
            .enumerated().compactMap { n, text in
                ResultCard(["type": "note", "item_id": "m\(n)", "text": text, "state": "saved",
                            "line": "Now always on: “\(text)” (FERB's understanding)"])
            }
        for open in [false, true] {
            let view = VStack(alignment: .leading, spacing: 6) {
                NativeMessageText("Now always on: 3 things (FERB's understanding)", size: 12)
                NoticeNotes(cards: cards, model: model, open: open)
            }
            try draw(view, width: 420, name: open ? "mac-grouped-notice-open.png" : "mac-grouped-notice.png")
        }
    }
}
