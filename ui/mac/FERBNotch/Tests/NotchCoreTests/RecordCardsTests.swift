import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// FERB's record cards (ORCH-563..ORCH-565, ORCH-561, DMN-554, APP-802, APP-803): each kind the daemon sends decodes from its
/// payload, keeps no secret, stands for exactly its line, is drawn, and survives the saved chat.
@MainActor
final class RecordCardsTests: XCTestCase {
    /// Payloads as the daemon builds them (ferb/daemon/areas/records.py, Supervisor._task_record/_tasks_card,
    /// cards.done_cards).
    static let task: [String: Any] = [
        "type": "task", "task_id": "t-1", "name": "Send the brief", "status": "done_unverified", "verified": false,
        "note": "Alex received it", "line": "📋 Send the brief: done, not all of it confirmed (couldn't confirm: Alex received it).",
        "questions": [["question": "Which folder?", "answer": "Studio"], ["question": "no answer"]],
        "cards": [["type": "send", "kind": "mailbox_send", "recipient": "Alex", "site": "mail.example"],
                  ["type": "submit", "site": "forms.example"], ["type": "hologram"]]]

    static let payloads: [[String: Any]] = [
        ["type": "reminders", "count": 2, "line": "⏰ Your reminders and timers (2): Tea; Call",
         "reminders": [["id": "t-1", "kind": "timer", "name": "Tea", "at": "2026-11-09T10:00:00+00:00"],
                       ["id": "r-1", "kind": "reminder", "name": "Call", "at": "2026-11-10T09:00:00+00:00"],
                       ["name": "no id"]]],
        ["type": "tasks", "count": 1, "line": "📋 Your tasks (1): 1 done", "tasks": [task.filter { $0.key != "line" }]],
        task,
        ["type": "memory", "count": 1, "line": "🧠 FERB remembers 1 thing; 2 forgotten (their contents aren't kept).",
         "items": [["id": "m-1", "kind": "fact", "subject": "Alex", "key": "email", "text": "Alex's email is a@x.example",
                    "origin": "owner_stated"], ["id": "m-2"]],
         "forgotten": [["at": "2026-11-08T10:00:00.123456+00:00", "items": 2]]],
        ["type": "permissions", "count": 1, "line": "🔐 Standing permissions: 1 send scope in force",
         "standing_sends": [["held_by": "t-9", "capability": "send", "identity": "owner", "account": "me@x.example",
                             "site": "", "recipients": ["a@x.example"], "any_text": false]],
         "known_destinations": [["kind": "email", "value": "a@x.example", "how": "confirmed", "approved": true],
                                ["kind": "email", "how": "named"]],
         "routine_questions_off_until": "2026-11-09T11:00:00+00:00"],
        ["type": "logins", "count": 1, "line": "🔑 Saved logins (1): shop.example — Shop",
         "owner_saved": [["place": "shop.example", "name": "Shop", "kind": "secret", "unusable": false],
                         ["problem": "the saved logins couldn't be read (OSError)"]],
         "ferb_own": [["name": "FERB mail", "account": "f…@hint.example", "sites": ["mail.example"]]]],
        ["type": "about", "line": "ℹ️ FERB 0.27.3; 1 of 2 permissions granted.", "version": ["ferb": "0.27.3"],
         "models": ["worker": "Codex w", "checker": "Codex c", "planner": "Codex p", "intro": "Codex i"],
         "permissions": [["id": "accessibility", "status": "granted"], ["id": "full_disk_access", "status": "denied"]]],
        ["type": "submit", "site": "forms.example"],
        ["type": "trash", "path": "/tmp/a.txt", "name": "a.txt"],
        ["type": "delete", "path": "/tmp/b.txt", "name": "b.txt"],
        ["type": "download", "path": "/tmp/c.pdf", "name": "c.pdf"],
    ]

    // decision: DMN-554, APP-803, ORCH-565
    func testEveryRecordKindDecodesItsRowsAndKeepsNoSecret() throws {
        let cards = try XCTUnwrap(ResultCard.list(Self.payloads))
        XCTAssertEqual(cards.map(\.type), ["reminders", "tasks", "task", "memory", "permissions", "logins", "about",
                                           "submit", "trash", "delete", "download"])
        XCTAssertEqual(cards[0].reminders?.map(\.isTimer), [true, false])        // a row without an id isn't one
        let task = try XCTUnwrap(cards[1].tasks?.first)
        XCTAssertEqual(task.taskID, "t-1")
        XCTAssertNil(task.line)
        XCTAssertEqual(cards[2].status, "done_unverified")
        XCTAssertEqual(cards[2].verified, false)
        XCTAssertEqual(cards[2].note, "Alex received it")
        XCTAssertEqual(cards[2].questions?.map(\.answer), ["Studio"])
        XCTAssertEqual(cards[2].records?.map(\.type), ["send", "submit"])         // a kind this build doesn't know: left out
        XCTAssertEqual(cards[2].records?.compactMap(\.recordLine).count, 2)
        XCTAssertEqual(cards[3].memoryItems?.map(\.text), ["Alex's email is a@x.example"])
        XCTAssertEqual(cards[3].memoryItems?.first?.saidByOwner, true)
        XCTAssertEqual(cards[3].forgotten?.first?.items, 2)
        XCTAssertNotNil(cards[3].forgotten?.first?.atDate)
        XCTAssertEqual(cards[4].standingSends?.first?.recipients, ["a@x.example"])
        XCTAssertNil(cards[4].standingSends?.first?.site)
        XCTAssertEqual(cards[4].knownDestinations?.map(\.approved), [true])
        XCTAssertEqual(cards[5].ownerLogins?.compactMap(\.name), ["Shop"])
        XCTAssertEqual(cards[5].ownerLogins?.compactMap(\.problem).count, 1)
        XCTAssertEqual(cards[5].ferbLogins?.first?.sites, ["mail.example"])
        XCTAssertEqual(cards[6].version, "0.27.3")
        XCTAssertEqual(cards[6].models?.map(\.role), ["planner", "worker", "checker", "intro"])
        XCTAssertEqual(cards[6].permissionRows?.filter(\.granted).count, 1)
        XCTAssertEqual(cards[7].site, "forms.example")
        XCTAssertEqual(cards[8...].map(\.fileURL?.lastPathComponent), ["a.txt", "b.txt", "c.pdf"])
        XCTAssertEqual(CardLayout.below(cards).count, cards.count)                 // each drawn on its own surface
        // The logins card keeps names and sites only: the account hint the daemon sends is never kept or saved.
        let saved = try XCTUnwrap(String(data: JSONEncoder().encode(cards[5]), encoding: .utf8))
        XCTAssertFalse(saved.contains("hint.example"))
        // A card without what makes it one isn't one.
        XCTAssertNil(ResultCard.list([["type": "task", "name": "no id"], ["type": "reminders"], ["type": "submit"],
                                      ["type": "trash"], ["type": "memory"], ["type": "logins"], ["type": "about"],
                                      ["type": "permissions"], ["type": "tasks"]]))
    }

    // decision: ORCH-329, DMN-554, APP-803
    func testEachRecordCardStandsForItsLineAndAKindThisBuildDoesntKnowKeepsIt() throws {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults("RecordCards"))
        model.session.draft = "what do you have on record?"
        let id = try XCTUnwrap(model.session.beginWork())
        model.conversation.begin(id: id, request: "what do you have on record?", sources: [])
        let lines = Self.payloads.compactMap { $0["line"] as? String }
        let newer = "📒 Your ledger (2): …"                               // a kind a newer daemon may send
        let text = (["Here they are."] + lines + [newer]).joined(separator: "\n")
        model.receiveChatEvent(["type": "reply", "request_id": "R", "text": text,
                                "cards": Self.payloads + [["type": "ledger", "line": newer]]], localID: id)
        let turn = try XCTUnwrap(model.conversation.turns.first { $0.id == id })
        XCTAssertEqual(turn.answer, text)                                     // the text stays for history and voice
        XCTAssertEqual(turn.cards?.count, Self.payloads.count)
        XCTAssertEqual(CardLayout.bubble(turn.answer, turn.cards ?? [], note: nil, lines: []),
                       "Here they are.\n" + newer)
    }

    // decision: APP-802
    /// Every record card the playground shows (the owner's review of them all at once) reads and draws with the real
    /// views, in today's chat and as a past day's record.
    func testEveryRecordCardDraws() throws {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults("RecordCardsDraw"))
        let entries = PlaygroundRecordCards.entries(download: FileManager.default.temporaryDirectory.appendingPathComponent("none.pdf"))
        let reply = PlaygroundRecordCards.reply(download: FileManager.default.temporaryDirectory.appendingPathComponent("none.pdf"))
        XCTAssertEqual(CardLayout.strip(reply.text, ResultCard.list(reply.cards) ?? []),
                       "Here is what FERB's records hold. Nothing here is real.")
        var kinds: Set<String> = []
        for entry in entries + Self.payloads.map({ PlaygroundRecordCards.Entry(title: "", when: "", payload: $0) }) {
            let card = try XCTUnwrap(ResultCard(entry.payload), entry.title)
            XCTAssertTrue(CardLayout.surfaces.contains(card.type), card.type)
            kinds.insert(card.type)
            for live in [true, false] {
                let view = ResultCardView(card: card, turnID: UUID(), model: model).environment(\.cardsLive, live)
                let hosting = NSHostingView(rootView: AnyView(view.frame(width: 460)))
                XCTAssertGreaterThan(hosting.fittingSize.height, 30, card.type)
            }
        }
        XCTAssertTrue(kinds.isSuperset(of: ["reminders", "tasks", "task", "memory", "permissions", "logins", "about",
                                            "submit", "trash", "delete", "download"]))
        // Empty lists draw too ("No reminders or timers", "No tasks in this chat yet", …).
        for empty: [String: Any] in [["type": "reminders", "reminders": [Any]()], ["type": "tasks", "tasks": [Any]()],
                                     ["type": "memory", "items": [Any](), "forgotten": [Any]()],
                                     ["type": "permissions", "standing_sends": [Any](), "known_destinations": [Any]()],
                                     ["type": "logins", "owner_saved": [Any](), "ferb_own": [Any]()]] {
            let card = try XCTUnwrap(ResultCard(empty))
            let hosting = NSHostingView(rootView: ResultCardView(card: card, turnID: UUID(), model: model).frame(width: 460))
            XCTAssertGreaterThan(hosting.fittingSize.height, 20, card.type)
        }
    }

    // decision: APP-801, APP-803, STORE-403
    func testRecordCardsAreSavedInTheChatAndReadBack() throws {
        var archive = ConversationArchive()
        let id = UUID()
        archive.begin(id: id, request: "what do you have on record?", sources: [], date: Date(timeIntervalSince1970: 1_794_000_000))
        archive.showCards(id: id, Self.payloads)
        archive.finish(id: id, answer: "Here.", status: "done", artifacts: [])
        let read = try ConversationArchive.read(JSONEncoder().encode(archive.forStorage()))
        XCTAssertEqual(read.dropped, 0, "\(read.reasons)")
        XCTAssertEqual(read.archive.turns.first?.cards, archive.turns.first?.cards)
        XCTAssertEqual(read.archive.turns.first?.cards?.first { $0.type == "task" }?.records?.count, 2)
    }
}
