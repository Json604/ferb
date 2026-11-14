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
        XCTAssertEqual(scenarios.count, 71)
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
        XCTAssertEqual(compared, 71, "every conversation of every scenario (preview_attention sends no message; login_sites has two)")
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

    private static func golden() throws -> [[String: Any]] {
        let repo = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent()
        let url = repo.appendingPathComponent("rust/fixtures/chat-entries/scenarios.json")
        let object = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any]
        return object?["scenarios"] as? [[String: Any]] ?? []
    }
}
