import XCTest
@testable import FERBNotch
@testable import NotchCore

/// The saved chat across builds (APP-801, STORE-403). 2026-11-08: a35676cf added `ResultCard.changed: Bool = false`,
/// synthesized Codable required the key, every card saved before it failed, and the first launch of that build set the
/// owner's whole 11-04..11-08 chat aside unread.
///
/// Fixtures/archive-<version>.json is an archive as that release saved it: the release's own NotchCore sources
/// (`git archive v<version> ui/mac/FERBNotch/Sources/NotchCore`) compiled with swiftc and a small program that builds a
/// chat through that build's API (a one-ask answer with every card type, a multi-part card with rows, a late reply,
/// a merged message, a runtime switch, an interrupted draft, a recovered task, notices with quotes, proposals and a
/// page card) and encodes `forStorage()` as saveConversation does. 0.27.3's NotchCore is the pre-a35676cf shape
/// (identical sources).
@MainActor
final class ArchiveUpgradeTests: XCTestCase {
    static let releases = ["0.27.0", "0.27.3"]

    func fixture(_ version: String) throws -> Data {
        let url = try XCTUnwrap(Bundle.module.url(forResource: "archive-" + version, withExtension: "json",
                                                  subdirectory: "Fixtures"))
        return try Data(contentsOf: url)
    }

    func object(_ data: Data) throws -> [String: Any] {
        try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    }

    // decision: APP-801, STORE-403
    func testEveryReleasedBuildsArchiveReadsInFull() throws {
        for version in Self.releases {
            let data = try fixture(version)
            let saved = try object(data)
            let read = try ConversationArchive.read(data)
            XCTAssertEqual(read.dropped, 0, "\(version): \(read.reasons)")
            let archive = read.archive
            XCTAssertEqual(archive.turns.count, (saved["turns"] as? [Any])?.count, version)
            XCTAssertEqual(archive.replies.count, (saved["replies"] as? [Any])?.count, version)
            XCTAssertEqual(archive.notices.count, (saved["notices"] as? [Any])?.count, version)
            // The plain decoder, as every caller used it, reads it too.
            XCTAssertEqual(try JSONDecoder().decode(ConversationArchive.self, from: data), archive, version)

            let first = archive.turns[0]
            XCTAssertEqual(first.cards?.map(\.type), ["event", "link", "file", "payment", "send", "reminder"], version)
            XCTAssertEqual(first.cards?.map(\.changed), Array(repeating: false, count: 6), version)
            XCTAssertEqual(first.receiptCards?.first?.state, "corrected", version)
            let card = try XCTUnwrap(archive.turns[1].card, version)
            XCTAssertEqual(card.rows.map(\.state), ["done", "failed"], version)
            XCTAssertEqual(card.line, "1 of 2 done", version)
            XCTAssertEqual(archive.turns.first { $0.requestID == "req-6" }?.runtimeSwitch?.choices, ["claude"], version)
            XCTAssertEqual(archive.replies.first?.part, .answer, version)
            XCTAssertEqual(archive.notices.first?.quotes.first?.origin, "Mail", version)
            XCTAssertEqual(archive.notices.last?.proposals?.first?.decision, "confirmed", version)
            XCTAssertEqual(archive.notices.last?.cards?.first?.page, "memory", version)
            // What this build saves from it reads back the same.
            let resaved = try JSONEncoder().encode(archive.forStorage())
            XCTAssertEqual(try ConversationArchive.read(resaved).archive, archive, version)
        }
    }

    // decision: APP-801, STORE-403
    func testAReleasedArchiveLoadsAtLaunchWithNothingSetAside() throws {
        let defaults = scratchDefaults()
        let data = try fixture("0.27.3")
        defaults.set(data, forKey: ArchiveStore.archiveKey)
        let model = NotchModel(preferences: defaults)
        XCTAssertEqual(model.conversation.turns.count, 8)
        XCTAssertFalse(model.archiveBlocked)
        XCTAssertEqual(asideKeys(defaults), [])
    }

    /// Every key a build may not have saved yet (an older one) is optional: without it the entry holding it reads in
    /// full. Only the keys that make an entry what it is may be required, and without one only that entry is left out.
    /// A stored property added later without tolerant reading (as `changed` was) fails here.
    // decision: APP-801, APP-803, STORE-403
    func testWithoutAnyOneKeyOnlyTheEntryItIdentifiesIsLost() throws {
        var archive = try ConversationArchive.read(fixture("0.27.3")).archive
        let id = UUID()
        archive.begin(id: id, request: "List my schedules", sources: [], date: Date(timeIntervalSince1970: 1_793_100_000))
        archive.showCards(id: id, [
            ["type": "schedules", "count": 1, "line": "1 schedule",
             "schedules": [["id": "s-1", "name": "Stretch", "repeat": "every day at 9:00", "state": "paused",
                            "next_run": "2026-10-28T09:00:00+00:00", "until": "2026-11-30T00:00:00+00:00",
                            "watch": "news"]]],
            ["type": "event", "kind": "schedule", "id": "s-1", "what": "Stretch", "changed": true,
             "was_repeat": "every day at 8:00", "was_request": "stretch", "request": "stretch for 5 minutes"],
        ] + RecordCardsTests.payloads)                       // FERB's record cards (APP-802), nested rows included
        archive.finish(id: id, answer: "1 schedule", status: "done", artifacts: [])
        XCTAssertEqual(try ConversationArchive.read(JSONEncoder().encode(archive)).archive, archive)
        let full = try object(JSONEncoder().encode(archive))
        let counts = [archive.turns.count, archive.replies.count, archive.notices.count]

        let identity: [String: Set<String>] = [
            "turns": ["id", "createdAt"], "replies": ["id", "turnID", "createdAt", "afterTurnID"],
            "notices": ["id", "entryID", "createdAt", "text"], "cards": ["type"], "receiptCards": ["type"],
            "card": ["taskID"], "rows": ["taskID", "part"], "question": ["approvalID"], "runtimeSwitch": ["choices"],
            "quotes": ["text"], "proposals": ["id", "text"], "schedules": ["id", "name", "repeatRule"],
            // FERB's record cards' rows (APP-802): a nested card is a card; each row needs only what makes it one
            "tasks": ["type"], "records": ["type"], "reminders": ["id"], "questions": ["question", "answer"],
            "memoryItems": ["text"], "forgotten": ["at"], "knownDestinations": ["value"], "ferbLogins": ["name"],
            "models": ["role", "model"], "permissionRows": ["id"],
        ]
        var checked = 0
        for path in keyPaths(full) where path.count > 1 {
            let owner = path.dropLast().last { $0 is String } as? String ?? ""
            let key = path.last as? String ?? ""
            let data = try JSONSerialization.data(withJSONObject: removing(path, from: full))
            let read = try ConversationArchive.read(data)
            let left = [read.archive.turns.count, read.archive.replies.count, read.archive.notices.count]
            let where_ = path.map { "\($0)" }.joined(separator: ".")
            if identity[owner]?.contains(key) == true {
                XCTAssertEqual(read.dropped, 1, where_)
                XCTAssertLessThanOrEqual(counts.reduce(0, +) - left.reduce(0, +), 1, where_)
            } else {
                XCTAssertEqual(read.dropped, 0, "\(where_): \(read.reasons)")
                XCTAssertEqual(left, counts, where_)
            }
            checked += 1
        }
        XCTAssertGreaterThan(checked, 250)   // the sweep reached the cards, rows, record rows and notices
    }

    // decision: APP-801
    func testANewerBuildsUnknownKeysAndValuesStillLoad() throws {
        var saved = try object(fixture("0.27.3"))
        saved["futureList"] = [1, 2]
        var turns = try XCTUnwrap(saved["turns"] as? [[String: Any]])
        turns[0]["futureKey"] = ["nested": true]
        var cards = try XCTUnwrap(turns[0]["cards"] as? [[String: Any]])
        cards[0]["futureFlag"] = 3
        cards.append(["type": "chart", "series": [1, 2, 3]])     // a card type this build doesn't draw
        turns[0]["cards"] = cards
        saved["turns"] = turns
        var replies = try XCTUnwrap(saved["replies"] as? [[String: Any]])
        replies[0]["part"] = "summary"                              // a reply part this build doesn't know
        saved["replies"] = replies
        let read = try ConversationArchive.read(JSONSerialization.data(withJSONObject: saved))
        XCTAssertEqual(read.archive.turns.count, 8)
        XCTAssertEqual(read.archive.replies.count, 1)
        XCTAssertNil(read.archive.replies[0].part)                  // shows the answer, as a reply saved before parts
        XCTAssertEqual(read.archive.turns[0].cards?.count, 7)
        XCTAssertEqual(read.dropped, 1)                             // the part, so the original is kept aside
    }

    // decision: APP-801, APP-569
    func testAnUnreadableEntryIsLeftOutAndTheOriginalKeptAside() throws {
        let defaults = scratchDefaults()
        var saved = try object(fixture("0.27.3"))
        var turns = try XCTUnwrap(saved["turns"] as? [[String: Any]])
        turns[3]["id"] = "not-a-uuid"
        saved["turns"] = turns
        let original = try JSONSerialization.data(withJSONObject: saved)
        defaults.set(original, forKey: ArchiveStore.archiveKey)

        let model = NotchModel(preferences: defaults)
        XCTAssertEqual(model.conversation.turns.count, 7)
        XCTAssertFalse(model.archiveBlocked)
        let aside = asideKeys(defaults)
        XCTAssertEqual(aside.count, 1)
        XCTAssertTrue(aside[0].hasPrefix(ArchiveStore.partialPrefix))
        XCTAssertEqual(defaults.data(forKey: aside[0]), original)
        // The readable rest is saved back, so the next launch reads it with nothing left out and sets nothing aside.
        XCTAssertEqual(try ConversationArchive.read(XCTUnwrap(defaults.data(forKey: ArchiveStore.archiveKey))).dropped, 0)
        XCTAssertEqual(NotchModel(preferences: defaults).conversation.turns.count, 7)
        XCTAssertEqual(asideKeys(defaults), aside)
    }

    // MARK: Recovery of a copy set aside unread

    /// The owner's state after 2026-11-08: the old chat set aside as unreadable, a newer chat saved live.
    func incident(_ defaults: UserDefaults) throws -> ConversationArchive {
        var live = ConversationArchive()
        for (index, request) in ["Good morning", "What's on today?"].enumerated() {
            let id = UUID()
            live.begin(id: id, request: request, sources: [], date: Date(timeIntervalSince1970: 1_794_000_000 + Double(index) * 60))
            _ = live.bindRequest(id: id, requestID: "live-\(index)")
            live.finish(id: id, answer: "Hi.", status: "done", artifacts: [])
        }
        live.addNotice(id: "live-notice", text: "Your timer finished.", at: Date(timeIntervalSince1970: 1_794_000_200))
        defaults.set(try JSONEncoder().encode(live), forKey: ArchiveStore.archiveKey)
        defaults.set(try fixture("0.27.3"), forKey: ArchiveStore.unreadablePrefix + "2026-11-08T02:42:50Z")
        return live
    }

    // decision: APP-801
    func testACopySetAsideUnreadIsMergedBackOnceAndKept() throws {
        let defaults = scratchDefaults()
        let live = try incident(defaults)
        let old = try ConversationArchive.read(fixture("0.27.3")).archive

        let model = NotchModel(preferences: defaults)
        let turns = model.conversation.turns
        XCTAssertEqual(turns.map(\.id), old.turns.map(\.id) + live.turns.map(\.id))
        XCTAssertEqual(turns.map(\.createdAt), turns.map(\.createdAt).sorted())
        XCTAssertEqual(Array(turns.suffix(2)), live.turns)                          // no live turn lost or changed
        XCTAssertEqual(model.conversation.replies.map(\.id), old.replies.map(\.id))
        XCTAssertEqual(model.conversation.notices.map(\.id), old.notices.map(\.id) + ["live-notice"])
        XCTAssertEqual(model.conversation.days.count, 2)                          // History shows both days again
        // Saved, and the copy renamed with its bytes unchanged, never deleted.
        let stored = try ConversationArchive.read(XCTUnwrap(defaults.data(forKey: ArchiveStore.archiveKey))).archive
        XCTAssertEqual(stored, model.conversation)
        XCTAssertEqual(asideKeys(defaults), [ArchiveStore.restoredPrefix + "2026-11-08T02:42:50Z"])
        XCTAssertEqual(defaults.data(forKey: ArchiveStore.restoredPrefix + "2026-11-08T02:42:50Z"), try fixture("0.27.3"))

        // A second launch changes nothing.
        let bytes = defaults.data(forKey: ArchiveStore.archiveKey)
        let again = NotchModel(preferences: defaults)
        XCTAssertEqual(again.conversation, model.conversation)
        XCTAssertEqual(defaults.data(forKey: ArchiveStore.archiveKey), bytes)
        XCTAssertEqual(asideKeys(defaults), [ArchiveStore.restoredPrefix + "2026-11-08T02:42:50Z"])
    }

    // decision: APP-801
    func testRecoveryNeverReplacesALiveEntry() throws {
        var old = try ConversationArchive.read(fixture("0.27.3")).archive
        var live = ConversationArchive()
        // The same turn id and the same request as an old turn, said differently here: the live ones stay.
        let shared = old.turns[0]
        live.begin(id: shared.id, request: "the live copy", sources: [], date: shared.createdAt)
        let other = UUID()
        live.begin(id: other, request: "the live request", sources: [], date: Date(timeIntervalSince1970: 1_793_000_100))
        _ = live.bindRequest(id: other, requestID: "req-2")
        old.addNotice(id: "only-old", text: "Old notice", at: Date(timeIntervalSince1970: 1_793_000_050))
        let added = live.restore(old)
        XCTAssertEqual(live.turns.first { $0.id == shared.id }?.request, "the live copy")
        XCTAssertEqual(live.turns.filter { $0.requestID == "req-2" }.map(\.id), [other])
        XCTAssertEqual(added, (old.turns.count - 2) + old.replies.count + old.notices.count)
        XCTAssertEqual(live.restore(old), 0)                                         // idempotent
        // An old entry made between two live ones goes between them.
        let turnTimes = live.turns.map(\.createdAt)
        XCTAssertEqual(turnTimes, turnTimes.sorted())
    }

    // decision: APP-801, APP-569
    func testACopyStillUnreadableStaysAsItIs() throws {
        let defaults = scratchDefaults()
        let garbage = Data("{\"turns\": \"not a list\"}".utf8)
        let key = ArchiveStore.unreadablePrefix + "2026-11-08T02:42:50Z"
        defaults.set(garbage, forKey: key)
        let model = NotchModel(preferences: defaults)
        XCTAssertTrue(model.conversation.turns.isEmpty)
        XCTAssertEqual(asideKeys(defaults), [key])
        XCTAssertEqual(defaults.data(forKey: key), garbage)
    }

    // MARK: Helpers

    /// This test's set-aside archive keys, sorted.
    func asideKeys(_ defaults: UserDefaults) -> [String] {
        defaults.dictionaryRepresentation().keys.filter { $0.hasPrefix(ArchiveStore.archiveKey + ".") }.sorted()
    }

    /// Every path to a key of a JSON object in `value` (array indexes as Int).
    func keyPaths(_ value: Any, _ prefix: [Any] = []) -> [[Any]] {
        if let object = value as? [String: Any] {
            return object.keys.sorted().flatMap { [prefix + [$0]] + keyPaths(object[$0]!, prefix + [$0]) }
        }
        if let array = value as? [Any] {
            return array.indices.flatMap { keyPaths(array[$0], prefix + [$0]) }
        }
        return []
    }

    /// `value` without the key at `path`.
    func removing(_ path: [Any], from value: Any) -> Any {
        guard let head = path.first else { return value }
        let rest = Array(path.dropFirst())
        if var object = value as? [String: Any], let key = head as? String {
            if rest.isEmpty { object.removeValue(forKey: key) } else { object[key] = removing(rest, from: object[key]!) }
            return object
        }
        if var array = value as? [Any], let index = head as? Int {
            array[index] = removing(rest, from: array[index])
            return array
        }
        return value
    }
}
