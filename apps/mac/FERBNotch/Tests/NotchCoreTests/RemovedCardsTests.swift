import XCTest
@testable import NotchCore

/// Owner 2026-11-10: the record cards of 2026-11-09 are gone (APP-823). Chats saved while FERB sent them, in FERB's
/// database (APP-805) and in the older archive, still read in full: a card of a type this build doesn't draw is kept as
/// saved (APP-801) but draws nothing, and the rest of its entry shows, the line it stood for included (APP-824,
/// STORE-403).
final class RemovedCardsTests: XCTestCase {
    /// One card of each removed type, as FERB.app saved them on 2026-11-09 (camelCase keys, nested rows included).
    static let removed: [[String: Any]] = [
        ["type": "reminders", "count": 1, "line": "⏰ Your reminders and timers (1): Stretch — Mon 09 Nov 18:00",
         "reminders": [["id": "t-1", "kind": "reminder", "name": "Stretch", "at": "2026-11-09T18:00:00+05:30"]]],
        ["type": "tasks", "count": 1, "line": "📋 Your tasks (1): 1 stopped",
         "tasks": [["type": "task", "taskID": "t1", "status": "cancelled", "name": "Join the Bridge application"]]],
        ["type": "task", "taskID": "28ae", "status": "cancelled", "verified": false, "changed": false,
         "line": "📋 Join the Bridge application: stopped.", "name": "Join the Bridge application",
         "questions": [["question": "FERB: this types an address… Go ahead?", "answer": "Yes"]],
         "records": [["type": "submit", "site": "apply.joinef.com"], ["type": "link", "host": "mail.google.com"]]],
        ["type": "memory", "count": 1, "line": "🧠 FERB remembers 1 thing.",
         "memoryItems": [["id": "m1", "kind": "fact", "text": "Prefers mornings"]], "forgotten": [["at": "x", "items": 1]]],
        ["type": "permissions", "count": 0, "line": "🔐 Standing permissions: 0 send scopes in force; 0 known destinations "
         + "(0 approved); routine questions on.", "standingSends": [], "knownDestinations": [["value": "a@b.c"]]],
        ["type": "logins", "count": 1, "line": "🔑 Saved logins (1): github.com — Work",
         "ownerLogins": [["place": "github.com", "name": "Work"]], "ferbLogins": [["name": "gmail"]]],
        ["type": "about", "line": "ℹ️ FERB 0.27.4; planner codex; 8 of 8 permissions granted.", "version": "0.27.4",
         "models": [["role": "planner", "model": "codex"]], "permissionRows": [["id": "accessibility"]]],
        ["type": "submit", "site": "apply.joinef.com", "changed": false],
        ["type": "trash", "path": "/tmp/old.txt", "name": "old.txt"],
        ["type": "delete", "path": "/tmp/gone.txt", "name": "gone.txt"],
        ["type": "download", "path": "/tmp/dl-1-Application.eml", "name": "dl-1-Application.eml"],
    ]
    static let answer = "Stopped, as you asked.\n📋 Join the Bridge application: stopped.\nSites contacted: 1 site"
    static let kept: [[String: Any]] = [["type": "link", "host": "apply.joinef.com", "line": "Sites contacted: 1 site"]]

    func turnBody(cards: [[String: Any]]) -> [String: Any] {
        ["id": UUID().uuidString, "request": "cancel the task", "status": "done", "answer": Self.answer,
         "createdAt": 815941526.3, "artifacts": [], "sources": [], "verified": false, "cards": cards,
         "receiptCards": cards]
    }

    // decision: APP-824, STORE-403
    func testASavedEntryWithEachRemovedCardTypeShowsWithoutThem() throws {
        for removed in Self.removed {
            let body = turnBody(cards: [removed] + Self.kept)
            // FERB's database (APP-805): each entry read on its own
            guard case let .turn(turn)? = HistoryEntry.read(["kind": "turn", "body": body]) else {
                return XCTFail("\(removed["type"] ?? "?"): the entry wasn't read")
            }
            let type = try XCTUnwrap(removed["type"] as? String)
            XCTAssertEqual(turn.cards?.map(\.type), [type, "link"], type)
            XCTAssertEqual(turn.receiptCards?.count, 2)
            XCTAssertEqual(turn.answer, Self.answer)
            // nothing draws it: not above, below, as a site or a file
            let cards = turn.cards ?? []
            XCTAssertEqual(CardLayout.above(cards) + CardLayout.below(cards) + CardLayout.files(cards), [], type)
            XCTAssertEqual(CardLayout.links(cards).map(\.host), ["apply.joinef.com"])
            // the removed card's line stays in the text; the link card still stands for its own line
            let shown = CardLayout.strip(turn.answer, turn.cards ?? [])
            XCTAssertEqual(shown, "Stopped, as you asked.\n📋 Join the Bridge application: stopped.")
            // the older archive: nothing left out, nothing set aside
            let archive: [String: Any] = ["turns": [body], "replies": [], "notices": [
                ["id": "n1", "entryID": UUID().uuidString, "createdAt": 815941530.0, "text": "notice", "quotes": [],
                 "cards": [removed]]]]
            let read = try ConversationArchive.read(JSONSerialization.data(withJSONObject: archive))
            XCTAssertEqual(read.dropped, 0, read.reasons.joined(separator: "; "))
            XCTAssertEqual(read.archive.turns.first?.cards?.count, 2)
            XCTAssertEqual(read.archive.notices.count, 1)
        }
        // the live event path never takes them either
        XCTAssertNil(ResultCard.list(Self.removed))
    }
}
