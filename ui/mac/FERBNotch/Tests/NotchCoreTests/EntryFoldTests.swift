import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Stage 2 (ENG-8): the engine's chat entries, folded, give the transcript FERB.app's own reducer builds today.
///
/// For each recorded scenario in `rust/fixtures/chat-entries/scenarios.json` (made by
/// `rust/crates/ferb-supervisor/tests/chat_entries.rs` from the Python daemon's recordings; the entries are NOT
/// Python-recorded: they are the engine's projection), the scenario's timeline is fed through the app's existing path
/// (a submitted turn as `begin` makes it, `receiveChatEvent` for each stream frame, the stream task's end and
/// `finishSubmission`, `receiveBusEvent`, `receiveOutcome` for a result `chat.status` read back, `receiveNotices`,
/// `receiveReminder`), and the golden entries are folded with `EntryFold`. The two transcripts must be equal on what
/// the owner sees: every entry in order, each turn's saved fields (its `Codable` form: words, statuses, plan, card and
/// checklist, cards, answer, question, merged, hidden kinds, sources), "Answered below", each late reply's turn,
/// the turn it follows and what it shows, each notice's text, quotes, tag and cards.
///
/// Left out of the comparison (timing or identity only): every id (a turn's, reply's or notice's UUID; replies and
/// notices are compared by the index of the turn they name), every `createdAt`, a notice's `noticedAt` (a reminder's
/// is the moment the app received it). Live state is not in the comparison because it is not in the archive's saved
/// form (live lines, typing dots, the streamed plan, step states, open questions).
///
/// The fixture is read from the repository by path, as other tests here read repository files (SoundFeedbackTests).
@MainActor
final class EntryFoldTests: XCTestCase {
    // decision: ENG-9
    func testEveryRecordedScenarioFoldsToTheTranscriptTheAppBuilds() throws {
        let scenarios = try Self.golden()
        XCTAssertEqual(scenarios.count, 72)          // 71 recorded, 1 invented (ENG-18)
        var compared = 0
        var differences: [String] = []
        for scenario in scenarios {
            let name = scenario["name"] as? String ?? "?"
            let timeline = scenario["timeline"] as? [[String: Any]] ?? []
            let entries = ChatEntry.list(scenario["entries"])
            let sessions = Set(timeline.compactMap { $0["session"] as? String })
            for session in sessions.sorted() {
                let app = Self.shape(appArchive(timeline.filter { $0["session"] as? String == session }))
                let folded = Self.shape(EntryFold.archive(entries.filter { $0.conversationID == session }))
                compared += 1
                if app != folded {
                    differences.append("\(name) [\(session)]:\n  app:  \(app)\n  fold: \(folded)")
                }
            }
        }
        XCTAssertEqual(compared, 72, "every conversation of every scenario (preview_attention sends no message; login_sites has two)")
        XCTAssertTrue(differences.isEmpty, differences.joined(separator: "\n"))
    }

    /// The comparison sees what the owner sees: a late answer put in place, a card row's state, a reminder dropped.
    // decision: ENG-9
    func testTheComparisonSeesAPlaceACardRowAndAPost() throws {
        let scenarios = try Self.golden()
        func entries(_ name: String) -> [[String: Any]] {
            scenarios.first { $0["name"] as? String == name }?["entries"] as? [[String: Any]] ?? []
        }
        func differs(_ name: String, _ change: (inout [[String: Any]]) -> Void) -> Bool {
            var changed = entries(name)
            change(&changed)
            return Self.shape(EntryFold.archive(json: entries(name))) != Self.shape(EntryFold.archive(json: changed))
        }
        XCTAssertTrue(differs("steer_running_task") { list in
            let i = list.firstIndex { $0["placement"] as? String == "below" }!
            list[i]["placement"] = "inline"
        })
        XCTAssertTrue(differs("several_asks") { list in
            let i = list.firstIndex { $0["kind"] as? String == "plan" }!
            var body = list[i]["body"] as! [String: Any]
            var card = body["card"] as! [String: Any]
            var rows = card["rows"] as! [[String: Any]]
            rows[1]["state"] = "failed"
            card["rows"] = rows
            body["card"] = card
            list[i]["body"] = body
        })
        XCTAssertTrue(differs("timer_set_and_fired") { list in list.removeAll { $0["kind"] as? String == "post" } })
        // a later answer dropped, or its words changed (APP-914)
        func isLater(_ entry: [String: Any]) -> Bool { (entry["body"] as? [String: Any])?["follows"] is String }
        XCTAssertTrue(differs("conversation_change") { list in list.remove(at: list.lastIndex(where: isLater)!) })
        XCTAssertTrue(differs("conversation_handover") { list in
            let i = list.lastIndex(where: isLater)!
            var body = list[i]["body"] as! [String: Any]
            body["text"] = "Something else."
            list[i]["body"] = body
        })
    }

    /// Every reply and the task's end of a turn are shown, in order, none overwritten (APP-914): the recorded scenarios
    /// where FERB said more after a turn's first answer, through the app's own reducer.
    // decision: APP-914
    func testEveryReplyOfATurnAndItsTaskEndAreShownInOrder() throws {
        let scenarios = try Self.golden()
        func said(_ name: String) -> [String] {
            let timeline = scenarios.first { $0["name"] as? String == name }?["timeline"] as? [[String: Any]] ?? []
            return appArchive(timeline).transcript().compactMap { entry -> String? in
                switch entry {
                case let .turn(turn, answeredBelow): return answeredBelow || turn.answer.isEmpty ? nil : turn.answer
                case let .reply(_, shown): return shown.answer
                case .notice: return nil
                }
            }
        }
        // a message with conversation changes: code's line for each change, then the planner's own reply
        XCTAssertEqual(said("conversation_change"), [
            "Paused the chat with Robin.", // ORCH-649: the engine names the chat, never its id
            "Blocked by a safety check: a conversation I'm having for you changes only on your own instruction. The "
                + "words I quoted from you aren't in this message or your recent messages in this chat, so that "
                + "conversation didn't change. Say what should change in it.",
            "No such delegated conversation",
            "Pausing the chat with Robin.",
            "Conversation handed back to you.\n\nI couldn't confirm part of this.",
        ])
        // a delegated conversation handed back while its task ran, then the task's end, checked
        XCTAssertEqual(said("conversation_handover"), [
            "Conversation handed over: Robin can swap seeds on Saturday.",
            "Robin can swap seeds on Saturday.",
        ])
        let timeline = scenarios.first { $0["name"] as? String == "conversation_handover" }?["timeline"]
            as? [[String: Any]] ?? []
        let archive = appArchive(timeline)
        XCTAssertEqual(archive.turns.count, 1)
        XCTAssertEqual(archive.turns[0].status, "done")
        let later = try XCTUnwrap(archive.replies.first?.said)
        XCTAssertTrue(later.verified)            // the task's end keeps its own check mark

        // where a later answer shows: right under its turn while that turn is the newest (no quote); after a newer
        // message, at the bottom quoting its turn, and the turn keeps its own answer (no "Answered below")
        var chat = ConversationArchive()
        let a = UUID(), b = UUID()
        chat.begin(id: a, request: "chat with Robin", sources: [])
        chat.finish(id: a, answer: "Handed over.", status: "done", artifacts: [])
        chat.finish(id: a, answer: "Robin says Saturday.", status: "done", artifacts: [])
        chat.begin(id: b, request: "what's the weather", sources: [])
        chat.finish(id: a, answer: "Robin confirmed.", status: "done", artifacts: [])
        XCTAssertEqual(chat.replies.map { chat.followsItsTurn($0) }, [true, false])
        XCTAssertEqual(chat.replies.map(\.afterTurnID), [a, b])
        guard case let .turn(turn, answeredBelow) = chat.transcript()[0] else { return XCTFail("the turn first") }
        XCTAssertEqual(turn.answer, "Handed over.")
        XCTAssertFalse(answeredBelow)
    }

    /// Live 2026-11-15 (APP-919): a message that stopped a task the owner had restarted with a second message, and asked
    /// for new work, showed the stopped task's summary three times: the stop message's receipt (ORCH-638), the task's
    /// own turn (APP-825), and a later answer of the task's FIRST turn at the bottom, because recordAnswer took the first
    /// turn bound to the task. The invented scenario through the app's own reducer, in both orders the engine can send
    /// the new task's plan and the stopped task's end: the summary is the second turn's answer and the stop message's
    /// receipt, and no reply says it again.
    // decision: APP-919
    // decision: ENG-18
    func testAStoppedTaskTwoTurnsRanTellsItsEndOnceOnTheTurnThatRanIt() throws {
        let scenarios = try Self.golden()
        let scenario = try XCTUnwrap(scenarios.first { $0["name"] as? String == "stop_beside_new_work_two_turns" })
        let timeline = scenario["timeline"] as? [[String: Any]] ?? []
        let plan = try XCTUnwrap(timeline.firstIndex {
            ($0["stream"] as? NSNumber)?.intValue == 2 && ($0["frame"] as? [String: Any])?["type"] as? String == "plan"
        })
        let summary = try XCTUnwrap((timeline[plan]["frame"] as? [String: Any])?["receipts"] as? [String]).first
        // the other order: the new task's plan after the stopped task's end and its stream's end
        var later = timeline
        let moved = later.remove(at: plan)
        let streamEnd = try XCTUnwrap(later.firstIndex {
            $0["do"] as? String == "end" && ($0["stream"] as? NSNumber)?.intValue == 1
        })
        later.insert(moved, at: streamEnd + 1)
        for order in [timeline, later] {
            let archive = appArchive(order)
            XCTAssertEqual(archive.turns.count, 3)
            XCTAssertEqual(archive.turns[0].answer, "Stopped, as you asked.\n\nSites contacted: 1 site")
            XCTAssertEqual(archive.turns[1].answer, summary)
            XCTAssertEqual(archive.turns[1].status, "cancelled")
            XCTAssertEqual(archive.turns[2].receipts, [summary].compactMap { $0 })
            XCTAssertEqual(archive.turns[2].planLine, "I'm listing every file the assignment workers made.")
            XCTAssertTrue(archive.turns[2].answer.hasPrefix("Two files"))
            // the first turn's own late answer (posted below when its first run ended) is the only reply
            XCTAssertEqual(archive.replies.count, 1)
            XCTAssertNil(archive.replies.first?.said)
            XCTAssertEqual(archive.replies.first?.turnID, archive.turns[0].id)
            let said = archive.transcript().compactMap { entry -> String? in
                if case let .reply(reply, shown) = entry { return reply.said?.answer ?? shown.answer }
                return nil
            }
            XCTAssertFalse(said.contains { $0 == summary }, "no reply says the stopped summary again")
        }
    }

    /// A result read back with no request and no session (after a relaunch) ends the task's newest turn, never an
    /// earlier one it ran under, which has its answer (APP-919).
    // decision: APP-919
    func testAResultWithNoRequestEndsTheNewestTurnOfItsTask() {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults())
        let first = UUID(), again = UUID()
        model.conversation.begin(id: first, request: "watch the build", sources: [])
        model.conversation.bind(id: first, taskID: "t1")
        model.conversation.finish(id: first, answer: "Stopped.", status: "cancelled", artifacts: [])
        model.conversation.begin(id: again, request: "start it again", sources: [])
        model.conversation.bind(id: again, taskID: "t1")
        model.receiveOutcome(["task_id": "t1", "status": "done", "answer": "The build passed."], announce: false,
                             recordEvent: false)
        XCTAssertEqual(model.conversation.turns.map(\.answer), ["Stopped.", "The build passed."])
        XCTAssertTrue(model.conversation.replies.allSatisfy { $0.said == nil })
    }

    /// A history saved with the extra copy (a later answer of the task's first turn, as builds before APP-919 saved it)
    /// is read back and drawn whole; the same end told again adds nothing.
    // decision: APP-919
    func testAHistorySavedWithTheExtraCopyIsReadBackWhole() throws {
        var chat = ConversationArchive()
        let first = UUID(), second = UUID(), stop = UUID()
        let summary = "Stopped, as you asked. This is where things stood:\n\nThe app is complete."
        chat.begin(id: first, request: "do the assignments", sources: [])
        chat.bind(id: first, taskID: "t1")
        chat.finish(id: first, answer: "Stopped, as you asked.", status: "cancelled", artifacts: [])
        chat.begin(id: second, request: "start again", sources: [])
        chat.bind(id: second, taskID: "t1")
        chat.begin(id: stop, request: "stop all the tasks", sources: [])
        chat.finish(id: second, answer: summary, status: "cancelled", artifacts: [])
        chat.answerLater(id: first, LaterAnswer(answer: summary, status: "cancelled"))   // what the old code saved
        let saved = try JSONDecoder().decode(ConversationArchive.self, from: JSONEncoder().encode(chat))
        XCTAssertEqual(saved.transcript().count, chat.transcript().count)
        XCTAssertEqual(saved.replies.compactMap(\.said?.answer), [summary])
        var again = saved
        again.finish(id: second, answer: summary, status: "cancelled", artifacts: [])
        XCTAssertEqual(again.replies.count, saved.replies.count)
    }

    // decision: ENG-8
    func testAnImportedEntryIsReadAsTheAppSavedIt() throws {
        // an archive as 0.27.3 saved it, imported entry by entry, folds back to the same transcript
        let url = try XCTUnwrap(Bundle.module.url(forResource: "archive-0.27.3", withExtension: "json", subdirectory: "Fixtures"))
        let saved = try JSONDecoder().decode(ConversationArchive.self, from: Data(contentsOf: url))
        var pos = 0
        var legacy: [[String: Any]] = []
        for stored in saved.storedEntries {
            pos += 1
            let entry = try HistoryEntry(stored)
            legacy.append(["id": "legacy-\(pos)", "kind": "legacy", "pos": pos, "rev": 1, "state": "done",
                           "body": ["kind": entry.wire["kind"] ?? "", "body": entry.wire["body"] ?? [:]]])
        }
        XCTAssertFalse(legacy.isEmpty)
        let folded = EntryFold.archive(json: legacy)
        XCTAssertEqual(Self.shape(folded), Self.shape(saved.forStorage()))
    }

    /// The engine's own interleaving (journal 72's open item): for the 61 scenarios ferbd replays through the real
    /// program, the inputs in the order the ENGINE fed its chat log (`rust/fixtures/chat-entries/engine_chat.json` and
    /// `engine_more.json`, written by `rust/crates/ferbd/tests/chat_replay.rs`: its real interleaving across the
    /// connections and the bus, which a recording does not keep) through FERB.app's own reducer, against the entries
    /// the engine stored, folded: what the app would show for the frames in that order is what the engine stored. For
    /// the 26 scenarios whose stored order scenarios.json can't check (two streams at once, a question beside a stream)
    /// this is the order check against the app.
    // decision: ENG-9
    // decision: ENG-15
    func testTheEnginesOwnInterleavingFoldsToWhatTheAppShows() throws {
        var compared = 0, differences: [String] = []
        var names: Set<String> = []
        for file in ["engine_chat.json", "engine_more.json"] {
            for scenario in try Self.fixture(file) {
                let name = scenario["name"] as? String ?? "?"
                names.insert(name)
                let timeline = scenario["timeline"] as? [[String: Any]] ?? []
                let entries = ChatEntry.list(scenario["entries"])
                for session in Set(timeline.compactMap { $0["session"] as? String }).sorted() {
                    let app = Self.shape(appArchive(timeline.filter { $0["session"] as? String == session }))
                    let folded = Self.shape(EntryFold.archive(entries.filter { $0.conversationID == session }))
                    compared += 1
                    if app != folded { differences.append("\(name) [\(session)]:\n  app:  \(app)\n  fold: \(folded)") }
                }
            }
        }
        XCTAssertEqual(names.count, 61, "28 chat and 33 more scenarios")
        XCTAssertGreaterThanOrEqual(compared, 61)
        XCTAssertTrue(differences.isEmpty, differences.joined(separator: "\n"))
    }

    // MARK: the app's path

    /// The timeline through FERB.app's reducer, as the app runs it.
    private func appArchive(_ timeline: [[String: Any]]) -> ConversationArchive {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults())
        var local: [Int: UUID] = [:]
        for item in timeline {
            let stream = (item["stream"] as? NSNumber)?.intValue ?? -1
            switch item["do"] as? String {
            case "send":
                // NotchModel.begin(request:outgoing:sources:keepComposer:), without the presentation and the stream
                let sources = (item["input_paths"] as? [String] ?? []).map { URL(fileURLWithPath: $0) }
                if let active = model.session.activeTaskID { model.workSessions[active] = model.session }
                var next = NotchSession()
                next.draft = item["text"] as? String ?? ""
                next.attach(sources)
                next.expand()
                model.session = next
                guard let id = model.session.beginWork() else { XCTFail("a turn that can't begin"); continue }
                model.conversation.begin(id: id, request: model.session.draft, sources: model.session.attachments,
                                         date: LiveProgress.time(item["at"]) ?? Date(), unredacted: true)
                model.session.clearComposer()
                model.workSessions[id] = model.session
                local[stream] = id
            case "frame":
                guard let id = local[stream], let frame = item["frame"] as? [String: Any] else { continue }
                model.receiveChatEvent(frame, localID: id)
            case "end":
                // submitToDaemon's tail: a provisional reply cut off, the live line, finishSubmission
                guard let id = local[stream] else { continue }
                if model.conversation.turns.contains(where: { $0.id == id && $0.isProvisional }) {
                    model.receiveChatEvent(["type": "reply_abort", "reason": "The stream ended before FERB confirmed its reply."],
                                           localID: id)
                    model.withWorkSession(id) { model.session.waitForOwner("The response was interrupted. It has not been confirmed.") }
                }
                model.conversation.clearLiveLine(id: id)
                model.finishSubmission(localID: id)
            case "bus":
                model.receiveBusEvent(["type": "event", "topic": item["topic"] ?? "", "data": item["data"] ?? [:],
                                       "at": item["at"] ?? ""])
            case "result":
                model.receiveOutcome(item["data"] as? [String: Any] ?? [:], announce: false, recordEvent: false)
            case "notice":
                model.receiveNotices([item["data"] as? [String: Any] ?? [:]])
            case "restart":
                // the engine's start-up pass after its restart (engine_*.json): what it changes (open questions
                // expired, cut work interrupted) the app reads from chat.status, whose questions aren't in the archive
                continue
            case "notification":
                guard let banner = BannerItem(item["item"] as? [String: Any] ?? [:]),
                      case .reminder(let id) = banner.target else { continue }
                model.receiveReminder(id: id, text: banner.body, card: banner.card)
            default:
                XCTFail("unknown timeline step \(item["do"] ?? "")")
            }
        }
        return model.conversation
    }

    // MARK: what the owner sees

    /// The transcript top to bottom, each entry as sorted-key JSON without its ids and times.
    static func shape(_ archive: ConversationArchive) -> [String] {
        let index = Dictionary(archive.turns.enumerated().map { ($1.id, $0) }, uniquingKeysWith: { first, _ in first })
        return archive.transcript().map { entry -> String in
            var out: [String: Any]
            switch entry {
            case let .turn(turn, answeredBelow):
                out = encoded(turn)
                out["id"] = nil
                out["createdAt"] = nil
                out["entry"] = "turn"
                out["answeredBelow"] = answeredBelow
            case let .reply(reply, _):
                out = ["entry": "reply", "part": reply.part?.rawValue ?? "", "turn": index[reply.turnID] ?? -1,
                       "after": index[reply.afterTurnID] ?? -1]
                if let said = reply.said { out["said"] = encoded(said) }      // a later answer's own words (APP-914)
            case let .notice(notice):
                out = encoded(notice)
                for key in ["entryID", "createdAt", "noticedAt", "afterTurnID"] { out[key] = nil }
                out["entry"] = "notice"
                out["after"] = notice.afterTurnID.flatMap { index[$0] } ?? -1
            }
            let data = (try? JSONSerialization.data(withJSONObject: out, options: [.sortedKeys])) ?? Data()
            return String(decoding: data, as: UTF8.self)
        }
    }

    private static func encoded<T: Encodable>(_ value: T) -> [String: Any] {
        guard let data = try? JSONEncoder().encode(value),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return [:] }
        return object
    }

    private static func golden() throws -> [[String: Any]] { try fixture("scenarios.json") }

    private static func fixture(_ file: String) throws -> [[String: Any]] {
        let repo = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent()
        let url = repo.appendingPathComponent("rust/fixtures/chat-entries/" + file)
        let object = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any]
        return object?["scenarios"] as? [[String: Any]] ?? []
    }
}
