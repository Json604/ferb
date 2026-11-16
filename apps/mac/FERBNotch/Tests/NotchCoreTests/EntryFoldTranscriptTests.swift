import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Stage 3, slice S6 (T-1206): what a phone must show for a conversation, written from FERB.app's own fold of the
/// engine's entries (`EntryFold`, which `EntryFoldTests` proves equal to the app's reducer) over every scenario of
/// `engine/fixtures/chat-entries` (`scenarios.json`, `engine_chat.json`, `engine_more.json`), in a platform-neutral
/// shape: `engine/fixtures/chat-entries/transcripts.json`, documented in that folder's README ("The phone's
/// transcript"). The phone's fold (ferb-mobile, S7) is tested against it.
///
/// `UPDATE_GOLDEN=1 swift test --filter EntryFoldTranscriptTests` writes it again (as the Rust goldens are written);
/// otherwise it is compared byte for byte, so it can't drift from the fold.
@MainActor
final class EntryFoldTranscriptTests: XCTestCase {
    static let files = ["scenarios.json", "engine_chat.json", "engine_more.json"]

    // decision: ENG-22
    // decision: ENG-9
    // decision: ENG-10
    func testThePhoneTranscriptsAreWhatTheMacFoldShows() throws {
        let made = try Self.golden()
        let url = Self.folder.appendingPathComponent("transcripts.json")
        if ProcessInfo.processInfo.environment["UPDATE_GOLDEN"] != nil {
            try Data(made.utf8).write(to: url)
        }
        let kept = try String(contentsOf: url, encoding: .utf8)
        guard kept != made else { return }
        let (a, b) = (kept.components(separatedBy: "\n"), made.components(separatedBy: "\n"))
        let line = (0..<min(a.count, b.count)).first { a[$0] != b[$0] } ?? min(a.count, b.count)
        XCTFail("transcripts.json differs from the fold at line \(line + 1); UPDATE_GOLDEN=1 writes it again, then "
                + "review the diff\n  kept: \(line < a.count ? a[line] : "(none)")\n  fold: \(line < b.count ? b[line] : "(none)")")
    }

    /// Two runs write the same bytes.
    // decision: ENG-22
    func testTheTranscriptsAreWrittenTheSameEveryTime() throws {
        XCTAssertEqual(try Self.golden(), try Self.golden())
    }

    /// What the new scenarios show (S6), read from the golden's own items: a gapped task's status and its "Not done"
    /// line, the sealed IDs under a message, legacy rows before new turns, a deleted turn leaving no hole and no
    /// dangling "Answered below", the fresh conversation's marker first, another client's turn in place, questions by
    /// state and where an open one shows.
    // decision: ORCH-651
    // decision: GATE-604
    // decision: ENG-12
    // decision: STORE-424
    // decision: STORE-427
    // decision: APP-916
    // decision: ENG-13
    // decision: APP-300
    // decision: ENG-22
    func testTheNewScenariosShowWhatTheMacShows() throws {
        func conversations(_ name: String) throws -> [[String: Any]] {
            let scenario = try XCTUnwrap(Self.scenarios("scenarios.json").first { $0["name"] as? String == name })
            return Self.transcript(ChatEntry.list(scenario["entries"]))
        }
        func items(_ name: String, _ index: Int = 0) throws -> [[String: Any]] {
            try conversations(name)[index]["items"] as? [[String: Any]] ?? []
        }
        // ORCH-651: delivered, with parts not done: the status, no check mark, the answer ending with code's line
        let gapped = try items("gaps_single_task")
        XCTAssertEqual(gapped.count, 1)
        XCTAssertEqual(gapped[0]["status"] as? String, "done_with_gaps")
        let answer = try XCTUnwrap(gapped[0]["answer"] as? [String: Any])
        XCTAssertEqual(answer["verified"] as? Bool, false)
        XCTAssertTrue((answer["text"] as? String ?? "").hasSuffix(
            "\n\nNot done: Five real participants took part in the test — Only you can bring real participants; none were invented."))
        let checklist = try XCTUnwrap(try items("gaps_checklist")[0]["checklist"] as? [String: Any])
        let lines = checklist["lines"] as? [[String: Any]] ?? []
        XCTAssertEqual(lines.map { $0["kind"] as? String }, ["done", "gaps", "done"])
        XCTAssertEqual(lines[1]["words"] as? String, "Parts not done")
        XCTAssertEqual(checklist["collapsed"] as? String, "2 of 3 steps done")
        let rows = try XCTUnwrap((try items("gaps_card_row")[0]["card"] as? [String: Any])?["rows"] as? [[String: Any]])
        XCTAssertEqual((rows[1]["shows"] as? [String: Any])?["words"] as? String, "Delivered, with parts not done")
        // GATE-604: each message's sealed IDs and the notice under it
        let sealed = try items("sealed_ids_in_message")
        XCTAssertEqual((sealed[0]["sealed"] as? [[String: Any]])?.compactMap { $0["name"] as? String }, ["PASSPORT"])
        XCTAssertEqual((sealed[1]["sealed"] as? [[String: Any]])?.compactMap { $0["name"] as? String }, ["PASSPORT 2"])
        XCTAssertTrue((sealed[0]["hidden_notice"] as? String ?? "").contains("kept it sealed as “PASSPORT”"))
        // ENG-12: the carried rows as the app saved them, then the new turn
        let legacy = try items("legacy_then_new")
        XCTAssertEqual(legacy.map { $0["item"] as? String }, ["turn", "turn", "reply", "notice", "turn"])
        XCTAssertEqual(legacy.map { $0["legacy"] as? Bool }, [true, true, true, true, false])
        XCTAssertEqual((legacy[2]["said"] as? [String: Any])?["text"] as? String, "Sam moved lunch to 1:30.")
        // STORE-424: the deleted turn leaves nothing; the late answer it was shown after stays, after its own turn
        let between = try items("deleted_turn_between")
        XCTAssertEqual(between.map { $0["item"] as? String }, ["turn", "reply", "notice", "turn"])
        XCTAssertEqual(between[0]["answered_below"] as? Bool, true)
        XCTAssertEqual(between[1]["after"] as? String, between[0]["entry"] as? String)
        XCTAssertFalse(between.contains { $0["request_id"] as? String == "r2" })
        // the last turn deleted while an earlier one worked: that turn's end is its own answer, in place
        let last = try items("deleted_last_turn")
        XCTAssertEqual(last.map { $0["item"] as? String }, ["turn"])
        XCTAssertEqual(last[0]["answered_below"] as? Bool, false)
        // STORE-427: the new conversation starts with the marker
        let fresh = try conversations("fresh_conversation")
        XCTAssertEqual(fresh.map { $0["conversation"] as? String }, ["s1", "s2"])
        let marker = try XCTUnwrap((fresh[1]["items"] as? [[String: Any]])?.first)
        XCTAssertEqual(marker["text"] as? String, StoredData.freshMarker)
        XCTAssertEqual(marker["after"] as? NSNull, NSNull())
        // APP-916: the phone's turn in its place, by its client
        let other = try items("turn_from_another_client")
        XCTAssertEqual(other.map { $0["client_id"] as? String }, [nil, "phone-1", nil])
        // ENG-13: answered, expired at start-up, answered again; none shown at the end
        let asked = try conversations("questions_answered_and_expired")[0]["questions"] as? [[String: Any]] ?? []
        XCTAssertEqual(asked.map { $0["state"] as? String }, ["answered", "expired", "answered"])
        XCTAssertTrue(asked.allSatisfy { $0["shown"] is NSNull })
        // APP-300: an open question a card row's step asked shows in that row; another at the chat's end
        let open = try conversations("question_left_open")[0]["questions"] as? [[String: Any]] ?? []
        XCTAssertEqual(open.map { $0["shown"] as? String }, ["card_row", "end_of_chat"])
        XCTAssertEqual((open[0]["row"] as? [String: Any])?["part"] as? String, "p2")
    }

    /// The rows carried from chat_history in `legacy_then_new` are what FERB.app writes: each body decodes as the app
    /// reads its history and encodes back to the same JSON (`StoredEntry.encoded`).
    // decision: ENG-12
    // decision: ENG-22
    func testTheLegacyRowsAreTheAppsOwnSavedForm() throws {
        let scenario = try XCTUnwrap(Self.scenarios("scenarios.json").first { $0["name"] as? String == "legacy_then_new" })
        let rows = (scenario["timeline"] as? [[String: Any]] ?? []).filter { $0["do"] as? String == "legacy" }
        XCTAssertEqual(rows.count, 4)
        for row in rows {
            let body = try XCTUnwrap(row["body"] as? [String: Any])
            let stored = try XCTUnwrap(HistoryEntry.read(["kind": row["kind"] ?? "", "body": body]))
            let again = try JSONSerialization.jsonObject(with: stored.encoded()) as? NSDictionary
            XCTAssertEqual(again, body as NSDictionary, "\(row["id"] ?? "")")
        }
    }

    // MARK: the golden

    static var folder: URL {
        var repo = URL(fileURLWithPath: #filePath)
        for _ in 0..<6 { repo = repo.deletingLastPathComponent() }
        return repo.appendingPathComponent("engine/fixtures/chat-entries")
    }

    static func scenarios(_ file: String) throws -> [[String: Any]] {
        let object = try JSONSerialization.jsonObject(with: Data(contentsOf: folder.appendingPathComponent(file)))
        return (object as? [String: Any])?["scenarios"] as? [[String: Any]] ?? []
    }

    /// The file: every scenario of the three fixture files, each conversation's transcript, one item per line.
    static func golden() throws -> String {
        var out = "{\"about\": " + json(
            "Generated by apps/mac/FERBNotch/Tests/NotchCoreTests/EntryFoldTranscriptTests.swift (UPDATE_GOLDEN=1) from "
            + "FERB.app's fold (EntryFold) of each scenario's stored entries in scenarios.json, engine_chat.json and "
            + "engine_more.json. What a phone shows, top to bottom; the shape is in README.md (The phone's transcript). "
            + "Not Python-recorded.") + ",\n\"scenarios\": [\n"
        var blocks: [String] = []
        for file in files {
            for scenario in try scenarios(file) {
                let conversations = transcript(ChatEntry.list(scenario["entries"]))
                var block = "{\"file\": \(json(file)), \"scenario\": \(json(scenario["name"] as? String ?? "")), \"conversations\": ["
                block += conversations.map { conversation -> String in
                    let items = (conversation["items"] as? [[String: Any]] ?? []).map { json($0) }
                    let questions = (conversation["questions"] as? [[String: Any]] ?? []).map { json($0) }
                    return "\n{\"conversation\": \(json(conversation["conversation"] as? String ?? "")), \"items\": [\n"
                        + items.joined(separator: ",\n") + "],\n\"questions\": [" + (questions.isEmpty ? "" : "\n")
                        + questions.joined(separator: ",\n") + "]}"
                }.joined(separator: ",")
                blocks.append(block + "]}")
            }
        }
        return out + blocks.joined(separator: ",\n") + "\n]}\n"
    }

    /// Compact JSON with sorted keys, slashes unescaped.
    static func json(_ value: Any) -> String {
        let data = (try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys, .withoutEscapingSlashes,
                                                                                 .fragmentsAllowed])) ?? Data()
        return String(decoding: data, as: UTF8.self)
    }

    /// Each conversation of the entries (by id), with what the Mac's fold shows and its questions.
    static func transcript(_ entries: [ChatEntry]) -> [[String: Any]] {
        Set(entries.map(\.conversationID)).sorted().map { conversation in
            let mine = entries.filter { $0.conversationID == conversation }
            return ["conversation": conversation, "items": items(mine), "questions": questions(mine)]
        }
    }

    // MARK: items

    /// Where each archive id came from: the entry that made it (a message's turn, a recovered turn's or a late
    /// answer's answer, a notice or post), or the legacy entry that holds the app's saved turn, reply or notice.
    private static func sources(_ entries: [ChatEntry]) -> [UUID: ChatEntry] {
        var made: [UUID: ChatEntry] = [:]
        for entry in EntryFoldTests.newest(entries) where !entry.deleted {
            made[EntryFold.uuid(entry.id)] = entry
            if entry.kind == "legacy", let body = entry.body["body"] as? [String: Any] {
                for key in ["id", "entryID"] {
                    if let text = body[key] as? String, let id = UUID(uuidString: text) { made[id] = entry }
                }
            }
        }
        return made
    }

    private static func items(_ entries: [ChatEntry]) -> [[String: Any]] {
        let archive = EntryFold.archive(entries)
        let made = sources(entries)
        func ref(_ id: UUID) -> Any { made[id]?.id ?? id.uuidString }
        return archive.transcript().map { shown -> [String: Any] in
            switch shown {
            case let .turn(turn, answeredBelow):
                var out = turnItem(turn, answeredBelow: answeredBelow)
                out["entry"] = ref(turn.id)
                out["legacy"] = made[turn.id]?.kind == "legacy"
                out["client_id"] = made[turn.id]?.clientID ?? NSNull()
                return out
            case let .reply(reply, _):
                return ["item": "reply", "entry": ref(reply.id), "legacy": made[reply.id]?.kind == "legacy",
                        "answers": ref(reply.turnID), "after": ref(reply.afterTurnID),
                        "part": reply.part?.rawValue ?? NSNull(), "quotes_request": !archive.followsItsTurn(reply),
                        "said": reply.said.map(said) ?? NSNull()]
            case let .notice(notice):
                let source = made[notice.entryID]
                return ["item": source?.kind == "post" ? "reminder" : "notice", "entry": ref(notice.entryID),
                        "legacy": source?.kind == "legacy", "ref": notice.id,
                        "after": notice.afterTurnID.map(ref) ?? NSNull(), "text": notice.text,
                        "quotes": notice.quotes.map { ["text": $0.text, "origin": $0.origin] },
                        "cards": cards(notice.cards)]
            }
        }
    }

    /// A turn: the owner's message (or FERB's own turn for a task this chat didn't send, APP-585), FERB's first reply
    /// (direct answers, receipts, the plan line), its card or checklist, its status and its answer.
    private static func turnItem(_ turn: ConversationTurn, answeredBelow: Bool) -> [String: Any] {
        let live = ["working", "needs_user"].contains(turn.status)
        var out: [String: Any] = [
            "item": "turn", "request_id": turn.requestID ?? NSNull(), "task_id": turn.backendTaskID ?? NSNull(),
            "from": turn.isRecovered ? "ferb" : "owner", "task_name": turn.taskName ?? NSNull(),
            "status": turn.status, "status_inferred": turn.statusInferred == true,
            "notice": turn.responseNotice ?? NSNull(), "merged_into": turn.mergedInto ?? NSNull(),
            "question": turn.question ?? NSNull(), "answered_below": answeredBelow,
            "answer": turn.answer.isEmpty && (turn.displayText ?? "").isEmpty ? NSNull() : answer(turn),
            "card": turn.card.map { card($0, live: live) } ?? NSNull(),
            "checklist": turn.checklist.map { checklist($0, live: live) } ?? NSNull(),
            "first_reply": firstReply(turn), "parts": turn.parts ?? [],
        ]
        if turn.isRecovered {
            out["goal"] = turn.taskGoal ?? ""
        } else {
            out["text"] = turn.request
            // A path as the entry stored it: `URL(fileURLWithPath:)` resolves a relative one against the test's
            // working folder, which would put this machine's folder in the golden (`relativePath` is the stored text;
            // for an absolute path it equals `path`). Artifacts below likewise.
            out["files"] = turn.sources.map(\.relativePath)
            out["hidden"] = turn.hiddenKinds ?? []
            out["sealed"] = (turn.sealed ?? []).map { ["kind": $0.kind, "name": $0.name] }
            out["hidden_notice"] = turn.hiddenNotice ?? NSNull()
        }
        return out
    }

    /// FERB's first bubble: what the Mac draws of the plan event (a card's direct answers are in its rows, ORCH-549).
    private static func firstReply(_ turn: ConversationTurn) -> Any {
        guard turn.plan != nil || turn.planLine != nil || turn.direct != nil || turn.receipts != nil else { return NSNull() }
        return ["says": turn.firstReplySays, "direct": turn.shownDirect, "receipts": turn.receipts ?? [],
                "receipt_cards": cards(turn.receiptCards), "plan_line": turn.shownPlanLine ?? NSNull(),
                "plan_lines": turn.planLines ?? [], "plan": turn.planLine == nil ? (turn.shownPlan ?? NSNull()) : NSNull()]
    }

    private static func answer(_ turn: ConversationTurn) -> [String: Any] {
        ["text": turn.answer, "display_text": turn.displayText ?? NSNull(), "verified": turn.isVerified,
         "artifacts": turn.artifacts.map(\.relativePath), "note": turn.note ?? NSNull(), "lines": turn.codeLines ?? NSNull(),
         "cards": cards(turn.cards), "runtime_switch": snake(turn.runtimeSwitch)]
    }

    /// A later answer's own words (APP-914).
    private static func said(_ later: LaterAnswer) -> [String: Any] {
        ["text": later.answer, "display_text": later.displayText ?? NSNull(), "status": later.status,
         "verified": later.verified, "artifacts": later.artifacts.map(\.relativePath), "note": later.note ?? NSNull(),
         "lines": later.codeLines ?? NSNull(), "cards": cards(later.cards), "runtime_switch": snake(later.runtimeSwitch)]
    }

    /// A card (ORCH-319): its rows with what each shows once read from history (`RowDisplay`: no live step states,
    /// which only `watch` carries), and its footer.
    private static func card(_ card: TurnCard, live: Bool) -> Any {
        guard var out = snake(card) as? [String: Any] else { return NSNull() }
        out["rows"] = card.rows.map { row -> Any in
            var shown = snake(row) as? [String: Any] ?? [:]
            let display = RowDisplay.make(row: row, steps: [:], rows: card.rows, live: live, plan: card.plan)
            shown["shows"] = ["kind": display.kind.rawValue, "words": display.words ?? NSNull()] as [String: Any]
            return shown
        }
        return out
    }

    /// A checklist (APP-894): its steps, each line as history shows it, and its collapsed line once the task ended.
    private static func checklist(_ checklist: StepChecklist, live: Bool) -> Any {
        guard var out = snake(checklist) as? [String: Any] else { return NSNull() }
        out["lines"] = checklist.steps.map { step -> [String: Any] in
            let line = ChecklistLine.make(step: step, state: nil, ended: checklist.ended[step.id],
                                          live: live && !checklist.finished)
            return ["step": step.id, "kind": line.kind.rawValue, "words": line.words ?? NSNull()] as [String: Any]
        }
        out["collapsed"] = checklist.finished
            ? ChecklistSummary.collapsed(done: checklist.doneCount(live: false), total: checklist.steps.count) : NSNull()
        return out
    }

    private static func cards(_ cards: [ResultCard]?) -> Any { cards.map { list in list.map { snake($0) } } ?? NSNull() }

    /// A value's saved form with snake_case keys.
    private static func snake<T: Encodable>(_ value: T?) -> Any {
        guard let value else { return NSNull() }
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        guard let data = try? encoder.encode(value), let object = try? JSONSerialization.jsonObject(with: data) else {
            return NSNull()
        }
        return object
    }

    // MARK: questions

    /// Every question entry (not deleted), in `pos` order, with its state and body as the engine keeps them, and where
    /// the Mac shows it: an open one in a card row of its turn when a row asks it (`rowPlacements`, APP-300), else at
    /// the end of today's chat (APP-581); one no longer open isn't shown.
    private static func questions(_ entries: [ChatEntry]) -> [[String: Any]] {
        let archive = EntryFold.archive(entries)
        let made = sources(entries)
        return EntryFoldTests.newest(entries).filter { $0.kind == "question" && !$0.deleted }.map { entry in
            var asked = entry.body
            asked["approval_id"] = entry.approvalID
            asked["task_id"] = entry.taskID
            let placed = DaemonApproval(asked).flatMap { archive.rowPlacements([$0])[$0.id] }
            return ["entry": entry.id, "approval_id": entry.approvalID ?? NSNull(),
                    "request_id": entry.requestID ?? NSNull(), "task_id": entry.taskID ?? NSNull(),
                    "state": entry.state, "open": entry.open, "body": entry.body,
                    "shown": entry.open ? (placed == nil ? "end_of_chat" : "card_row") : NSNull(),
                    "row": entry.open ? placed.map { place -> Any in
                        ["turn": made[place.turnID]?.id ?? "", "part": place.part] } ?? NSNull() : NSNull()]
        }
    }
}
