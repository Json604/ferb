import XCTest
@testable import FERBNotch
@testable import NotchCore

/// L6, L7, L8 and the check mark, driven by events shaped exactly as the event contract
/// (`ack`, `plan`, `progress`, `reply`, `outcome`, each with `request_id` and `at`).
@MainActor
final class ChatFlowTests: XCTestCase {
    // decision: APP-579
    func testAckRendersAtOnceBeforeAnyOtherEvent() throws {
        let model = makeModel()
        let id = begin("Find my notes from yesterday", in: model)
        model.receiveChatEvent(Contract.ack("req-a", "On it."), localID: id)

        guard case let .turn(turn, answeredBelow) = try XCTUnwrap(model.visibleEntries.last) else {
            return XCTFail("The request turn should be the last entry")
        }
        XCTAssertEqual(turn.id, id)
        XCTAssertEqual(turn.ferbLines, ["On it."])
        XCTAssertEqual(turn.requestID, "req-a")
        XCTAssertNil(turn.plan)
        XCTAssertNil(turn.liveLine)
        XCTAssertEqual(turn.answer, "")
        XCTAssertEqual(turn.status, "working")
        XCTAssertFalse(answeredBelow)

        model.receiveChatEvent(Contract.plan("req-a", task: "task-a", text: "Looking through Notes for yesterday's notes."), localID: id)
        XCTAssertEqual(model.conversation.turns.first?.ferbLines, ["On it.", "Looking through Notes for yesterday's notes."])
        // The plan line is a chat line now; it is no longer filed in the activity feed (spec §4.7).
        XCTAssertFalse(model.session.activity.contains { $0.title == "Looking through Notes for yesterday's notes." })
        // History keeps the lines the owner saw.
        XCTAssertEqual(try savedArchive(model).turns.first?.ferbLines, ["On it.", "Looking through Notes for yesterday's notes."])
    }

    // decision: APP-579
    func testProgressUpdatesOneLiveLineInPlaceAndClearsOnOutcome() throws {
        let model = makeModel()
        let id = begin("Summarise the GitHub issue", in: model)
        model.receiveChatEvent(Contract.ack("req-a", "Got it."), localID: id)
        model.receiveChatEvent(Contract.plan("req-a", task: "task-a", text: "Reading the issue and summarising it."), localID: id)
        // An ordinary step never reaches the chat (owner, 2026-10-24: only the "N active tasks" row stays).
        model.receiveChatEvent(Contract.progress("req-a", task: "task-a", text: "opening www.instagram.com",
                                                 startedAt: "2026-10-13T23:14:20+05:30"), localID: id)
        XCTAssertNil(model.conversation.turns.first?.liveLine)
        // The AI service connection's lines do.
        model.receiveChatEvent(Contract.progress("req-a", task: "task-a", text: "reading github.com",
                                                 startedAt: "2026-10-13T23:14:29.123456+05:30", connection: "lost"),
                               localID: id)

        let first = try XCTUnwrap(model.conversation.turns.first?.liveLine)
        XCTAssertEqual(first.text, "reading github.com")
        XCTAssertEqual(first.startedAt.timeIntervalSince1970, utc(2026, 10, 13, 17, 44, 29.123456).timeIntervalSince1970, accuracy: 0.001)
        XCTAssertEqual(first.elapsedText(at: first.startedAt.addingTimeInterval(12)), "12s")

        model.receiveChatEvent(Contract.progress("req-a", task: "task-a", text: "writing summary.md",
                                                 startedAt: "2026-10-13T23:15:02+05:30", connection: "back"), localID: id)
        let updated = model.conversation.turns[0]
        XCTAssertEqual(updated.liveLine?.text, "writing summary.md")
        XCTAssertEqual(updated.liveLine?.startedAt, utc(2026, 10, 13, 17, 45, 2))
        XCTAssertEqual(updated.ferbLines, ["Got it.", "Reading the issue and summarising it."])
        XCTAssertEqual(model.visibleEntries.count, 1)
        // The live line is live state: it never enters history.
        let encoded = try JSONDecoder().decode(ConversationArchive.self, from: JSONEncoder().encode(model.conversation))
        XCTAssertNil(encoded.turns.first?.liveLine)
        XCTAssertEqual(encoded.turns.first?.ferbLines, ["Got it.", "Reading the issue and summarising it."])

        // Without `started_at` the step's time falls back to the event's `at`.
        var noStart = Contract.progress("req-a", task: "task-a", text: "looking at Brave", startedAt: "",
                                        connection: "lost")
        noStart.removeValue(forKey: "started_at")
        model.receiveChatEvent(noStart, localID: id)
        XCTAssertEqual(model.conversation.turns[0].liveLine?.startedAt, utc(2026, 10, 13, 17, 44, 29.5))

        model.receiveChatEvent(Contract.outcome("req-a", task: "task-a", answer: "The issue asks for a retry flag.",
                                                display: "Summary saved to summary.md."), localID: id)
        XCTAssertNil(model.conversation.turns[0].liveLine)
        XCTAssertEqual(model.conversation.turns[0].visibleAnswer, "Summary saved to summary.md.")
        // A step reported after the answer never brings the line back.
        model.conversation.showProgress(id: id, text: "late step", startedAt: Date())
        XCTAssertNil(model.conversation.turns[0].liveLine)
    }

    // decision: APP-579
    func testLiveLineElapsedTimeReadsInSecondsMinutesAndHours() {
        let start = Date(timeIntervalSince1970: 1_000)
        let line = LiveProgress(text: "reading github.com", startedAt: start)
        XCTAssertEqual(line.elapsedText(at: start), "0s")
        XCTAssertEqual(line.elapsedText(at: start + 12), "12s")
        XCTAssertEqual(line.elapsedText(at: start + 65), "1m 05s")
        XCTAssertEqual(line.elapsedText(at: start + 3_720), "1h 02m")
        XCTAssertEqual(line.elapsedText(at: start - 5), "0s")
    }

    // decision: APP-580
    func testAnswerToAArrivingAfterBIsABottomReplyQuotingAWithATurnMarked() throws {
        let model = makeModel()
        let a = begin("Find my notes\nfrom yesterday", in: model)
        model.receiveChatEvent(Contract.ack("req-a", "On it."), localID: a)
        model.receiveChatEvent(Contract.plan("req-a", task: "task-a", text: "Searching Notes."), localID: a)
        let b = beginAnother("What's 2 + 2?", in: model)
        model.receiveChatEvent(Contract.ack("req-b", "Sure."), localID: b)
        model.receiveChatEvent(Contract.reply("req-b", text: "4"), localID: b)
        // B is the newest entry, so its answer stays in place.
        XCTAssertEqual(layout(model), ["turn: Find my notes from yesterday", "turn: What's 2 + 2? = 4"])

        model.receiveChatEvent(Contract.outcome("req-a", task: "task-a", answer: "Found 3 notes.", verified: true), localID: a)
        XCTAssertEqual(layout(model), [
            "turn: Find my notes from yesterday (answered below)",
            "turn: What's 2 + 2? = 4",
            "reply to: Find my notes from yesterday = Found 3 notes."
        ])
        guard case let .reply(reply, answering) = try XCTUnwrap(model.visibleEntries.last) else {
            return XCTFail("The late answer should be the last entry")
        }
        XCTAssertEqual(reply.turnID, a)
        XCTAssertEqual(answering.id, a)
        XCTAssertEqual(answering.quote, "Find my notes from yesterday")
        XCTAssertTrue(answering.isVerified)

        // History keeps both entries, in the same order.
        let restored = try savedArchive(model)
        XCTAssertEqual(restored.transcript().map(\.id), model.visibleEntries.map(\.id))
        XCTAssertEqual(restored.transcript(on: Date()).count, 3)
        // The status poll re-delivers finished results; that never adds a second reply.
        model.receiveOutcome(["task_id": "task-a", "request_id": "req-a", "status": "done", "answer": "Found 3 notes.",
                              "verified": true], announce: false, recordEvent: false)
        XCTAssertEqual(model.visibleEntries.count, 3)
    }

    // decision: APP-580
    func testLateConversationalReplyAlsoGoesToTheBottom() {
        let model = makeModel()
        let a = begin("Is it raining in Delhi?", in: model)
        _ = beginAnother("And tomorrow?", in: model)
        model.receiveChatEvent(Contract.reply("req-a", text: "Light rain this evening."), localID: a)
        XCTAssertEqual(layout(model), [
            "turn: Is it raining in Delhi? (answered below)",
            "turn: And tomorrow?",
            "reply to: Is it raining in Delhi? = Light rain this evening."
        ])
    }

    // decision: APP-580
    func testCheckMarkShowsOnlyForVerifiedOutcomes() throws {
        let model = makeModel()
        let verified = begin("Rename the report", in: model)
        model.receiveChatEvent(Contract.plan("req-a", task: "task-a", text: "Renaming it."), localID: verified)
        model.receiveChatEvent(Contract.outcome("req-a", task: "task-a", answer: "Renamed.", verified: true), localID: verified)
        let unverified = beginAnother("Post the update", in: model)
        model.receiveChatEvent(Contract.plan("req-b", task: "task-b", text: "Posting it."), localID: unverified)
        model.receiveChatEvent(Contract.outcome("req-b", task: "task-b", status: "done_unverified", answer: "Posted.",
                                                verified: false), localID: unverified)

        XCTAssertTrue(turn(verified, in: model).isVerified)
        XCTAssertFalse(turn(unverified, in: model).isVerified)
        XCTAssertTrue(try XCTUnwrap(savedArchive(model).turns.first { $0.id == verified }).isVerified)
    }

    // decision: APP-578, APP-577
    func testTwoRequestsInFlightKeepTheirOwnAckPlanAndLiveLine() {
        let model = makeModel()
        let a = begin("Book a table for two", in: model)
        let b = beginAnother("Draft the launch post", in: model)
        model.receiveChatEvent(Contract.ack("req-b", "Okay."), localID: b)
        model.receiveChatEvent(Contract.ack("req-a", "On it."), localID: a)
        model.receiveChatEvent(Contract.plan("req-a", task: "task-a", text: "Checking OpenTable."), localID: a)
        model.receiveChatEvent(Contract.plan("req-b", task: "task-b", text: "Writing a draft."), localID: b)
        model.receiveChatEvent(Contract.progress("req-b", task: "task-b", text: "writing post.md",
                                                 startedAt: "2026-10-13T23:14:29+05:30", connection: "lost"), localID: b)
        model.receiveChatEvent(Contract.progress("req-a", task: "task-a", text: "reading opentable.com",
                                                 startedAt: "2026-10-13T23:14:30+05:30", connection: "lost"), localID: a)
        // An event naming B's request never lands on A's turn, even on A's stream.
        model.receiveChatEvent(Contract.progress("req-b", task: "task-b", text: "looking at Brave",
                                                 startedAt: "2026-10-13T23:14:31+05:30", connection: "lost"), localID: a)

        let turnA = turn(a, in: model), turnB = turn(b, in: model)
        XCTAssertEqual(turnA.requestID, "req-a")
        XCTAssertEqual(turnA.ferbLines, ["On it.", "Checking OpenTable."])
        XCTAssertEqual(turnA.liveLine?.text, "reading opentable.com")
        XCTAssertEqual(turnB.requestID, "req-b")
        XCTAssertEqual(turnB.ferbLines, ["Okay.", "Writing a draft."])
        XCTAssertEqual(turnB.liveLine?.text, "writing post.md")
    }

    // decision: APP-578, APP-580
    func testResultNamingARequestFindsItsTurnAfterTheStreamDroppedBeforeThePlan() {
        let model = makeModel()
        let a = begin("Export the photos", in: model)
        model.receiveChatEvent(Contract.ack("req-a", "On it."), localID: a)
        model.finishSubmission(localID: a)
        XCTAssertEqual(turn(a, in: model).status, "interrupted")
        _ = beginAnother("Thanks", in: model)

        model.receiveBusEvent(["topic": "task.result", "at": Contract.at, "data": [
            "request_id": "req-a", "task_id": "task-a", "status": "done", "answer": "Exported 40 photos.", "verified": false
        ] as [String: Any]])
        XCTAssertEqual(turn(a, in: model).answer, "Exported 40 photos.")
        XCTAssertEqual(layout(model), [
            "turn: Export the photos (answered below)",
            "turn: Thanks",
            "reply to: Export the photos = Exported 40 photos."
        ])
    }

    // decision: APP-579
    func testLiveLineLeavesWhenTheStreamEndsWithoutAnAnswer() async throws {
        let rpc = ScriptedChatRPC(events: [
            Contract.ack("req-a", "On it."),
            Contract.plan("req-a", task: "task-a", text: "Watching the build."),
            Contract.progress("req-a", task: "task-a", text: "running a command", startedAt: Contract.at)
        ])
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults())
        model.session.draft = "Watch the build"
        model.startWork()
        let id = try XCTUnwrap(model.conversation.turns.first?.id)
        await model.submissions[id]?.value
        let turn = try XCTUnwrap(model.conversation.turns.first)
        XCTAssertEqual(turn.ferbLines, ["On it.", "Watching the build."])
        XCTAssertNil(turn.liveLine)
        model.disconnectDaemon()
    }

    // decision: APP-569
    func testArchivesSavedBeforeChatFlowFieldsStillDecode() throws {
        var archive = ConversationArchive()
        archive.begin(id: UUID(), request: "Older request", sources: [])
        archive.finish(id: archive.turns[0].id, answer: "Older answer", status: "done", artifacts: [])
        var object = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(archive)) as? [String: Any])
        object.removeValue(forKey: "replies")
        var turns = try XCTUnwrap(object["turns"] as? [[String: Any]])
        for key in ["requestID", "ack", "plan", "verified"] { turns[0].removeValue(forKey: key) }
        object["turns"] = turns
        let restored = try JSONDecoder().decode(ConversationArchive.self, from: JSONSerialization.data(withJSONObject: object))
        XCTAssertEqual(restored.turns.first?.answer, "Older answer")
        XCTAssertTrue(restored.replies.isEmpty)
        XCTAssertFalse(restored.turns[0].isVerified)
    }

    // decision: APP-787
    func testTheClosedNotchShowsATasksReplyOnlyOnceFERBAcceptedIt() {
        // Owner 2026-11-07: a reply the planner withdrew (its plan sent back for repair) flashed in the closed notch;
        // "make sure tha tthis also doesnt show in the temo acknowledgement notch response for the task".
        let model = makeModel()
        let task = begin("fill both applications", in: model)
        model.collapse()
        model.receiveChatEvent(["type": "plan_delta", "request_id": "req-t", "at": Contract.at,
                                "text": "I'm filling both now."], localID: task)
        XCTAssertNil(model.acknowledgement.text, "a streaming task reply never reaches the closed notch")
        model.receiveChatEvent(["type": "plan_abort", "request_id": "req-t", "at": Contract.at,
                                "reason": "The provisional plan needs correction."], localID: task)
        XCTAssertNil(model.acknowledgement.text)
        model.receiveChatEvent(Contract.plan("req-t", task: "task-1", text: "I'm filling both, leaving them unsubmitted."),
                               localID: task)
        XCTAssertEqual(model.acknowledgement.text, "I'm filling both, leaving them unsubmitted.")   // the accepted reply
    }

    // decision: APP-714, APP-578
    func testASteeringTurnsOneReplyShowsOnce() {
        // Live 2026-11-04: a message that corrected a running task streamed the planner's sentence as a plan line
        // (plan_delta, plan_end) and ended with the steer's single `reply`; the streamed draft stayed beside the answer.
        let model = makeModel()
        let task = begin("Improve my resume", in: model)
        model.receiveChatEvent(Contract.plan("req-t", task: "task-r", text: "Improving your resume."), localID: task)
        let steer = beginAnother("Save it as a new file", in: model)
        model.receiveChatEvent(Contract.ack("req-s", "Starting on it."), localID: steer)
        for text in ["I'll keep the original ", "and save a copy."] {
            model.receiveChatEvent(["type": "plan_delta", "request_id": "req-s", "at": Contract.at, "text": text], localID: steer)
        }
        model.receiveChatEvent(["type": "plan_end", "request_id": "req-s", "at": Contract.at], localID: steer)
        model.receiveChatEvent(["type": "reply", "request_id": "req-s", "at": Contract.at, "task_id": "task-r",
                                "text": "I'll keep the original and save a copy."], localID: steer)
        // The streamed draft is a bubble of its own while it is set (FirstReplyView); the answer is the one reply.
        XCTAssertNil(turn(steer, in: model).streamingPlan)
        XCTAssertEqual(layout(model), ["turn: Improve my resume",
                                       "turn: Save it as a new file = I'll keep the original and save a copy."])
    }

    // decision: APP-712, APP-713, APP-569
    func testAPastedDraftWithPasteMarkersShowsTheCleanTextInTheBubbleAndHistory() async throws {
        // The ack as 0.27.1's daemon sent it: the composer's own brackets echoed back in `message`.
        let echoed = "\u{1b}[200~Keep a notes file\u{1b}[201~"
        let rpc = ScriptedChatRPC(events: [
            ["type": "ack", "request_id": "req-p", "at": Contract.at, "text": "", "message": echoed],
            Contract.reply("req-p", text: "Noted.")
        ])
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults())
        let copied = "[200~Keep a notes file[201~"                 // copied out of a terminal
        model.pasteProvenance.paste(copied, over: NSRange(location: 0, length: 0), in: "")
        model.session.draft = copied
        model.startWork()
        let id = try XCTUnwrap(model.conversation.turns.first?.id)
        XCTAssertEqual(turn(id, in: model).request, "Keep a notes file")
        await model.submissions[id]?.value
        XCTAssertEqual(rpc.sent, [echoed])                          // still marked as pasted, nothing leaked inside
        XCTAssertEqual(turn(id, in: model).request, "Keep a notes file")
        let saved = String(decoding: try XCTUnwrap(model.preferences.data(forKey: "FERBConversationArchive")), as: UTF8.self)
        XCTAssertTrue(saved.contains("Keep a notes file"))
        XCTAssertFalse(saved.contains("[20"))
        model.disconnectDaemon()
    }

    // MARK: - Support

    private func makeModel() -> NotchModel {
        NotchModel(preferences: scratchDefaults())
    }

    /// A submitted turn whose stream is the foreground session.
    private func begin(_ request: String, in model: NotchModel) -> UUID {
        model.session.draft = request
        let id = model.session.beginWork()!
        model.conversation.begin(id: id, request: request, sources: [])
        return id
    }

    /// A later turn; any running turn keeps streaming in the background.
    private func beginAnother(_ request: String, in model: NotchModel) -> UUID {
        if let active = model.session.activeTaskID { model.workSessions[active] = model.session }
        model.session = NotchSession()
        return begin(request, in: model)
    }

    private func turn(_ id: UUID, in model: NotchModel) -> ConversationTurn {
        model.conversation.turns.first { $0.id == id }!
    }

    private func savedArchive(_ model: NotchModel) throws -> ConversationArchive {
        try JSONDecoder().decode(ConversationArchive.self, from: XCTUnwrap(model.preferences.data(forKey: "FERBConversationArchive")))
    }

    /// The transcript as the owner reads it, top to bottom.
    private func layout(_ model: NotchModel) -> [String] {
        model.visibleEntries.map { entry in
            switch entry {
            case let .turn(turn, answeredBelow):
                let request = "turn: " + turn.quote
                if answeredBelow { return request + " (answered below)" }
                return turn.visibleAnswer.isEmpty ? request : request + " = " + turn.visibleAnswer
            case let .reply(_, answering):
                return "reply to: " + answering.quote + " = " + answering.visibleAnswer
            case let .notice(notice):
                return "notice: " + notice.text
            }
        }
    }

    private func utc(_ year: Int, _ month: Int, _ day: Int, _ hour: Int, _ minute: Int, _ second: Double) -> Date {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC")!
        let whole = calendar.date(from: DateComponents(year: year, month: month, day: day, hour: hour, minute: minute,
                                                       second: Int(second)))!
        return whole.addingTimeInterval(second - Double(Int(second)))
    }
}

/// Events with the contract's exact field names.
private enum Contract {
    static let at = "2026-10-13T23:14:29.5+05:30"

    static func ack(_ request: String, _ text: String) -> [String: Any] {
        ["type": "ack", "request_id": request, "at": at, "text": text]
    }

    static func plan(_ request: String, task: String, text: String) -> [String: Any] {
        ["type": "plan", "request_id": request, "at": at, "task_id": task, "text": text, "goal": text]
    }

    /// `connection`: a line about the AI service connection ("lost" / "back"); without it, an ordinary step.
    static func progress(_ request: String, task: String, text: String, startedAt: String,
                         connection: String? = nil) -> [String: Any] {
        var event: [String: Any] = ["type": "progress", "request_id": request, "at": at, "task_id": task, "text": text,
                                    "started_at": startedAt]
        if let connection { event["connection"] = connection }
        return event
    }

    static func reply(_ request: String, text: String) -> [String: Any] {
        ["type": "reply", "request_id": request, "at": at, "text": text, "display_text": text]
    }

    static func outcome(_ request: String, task: String, status: String = "done", answer: String, display: String? = nil,
                        verified: Bool = false) -> [String: Any] {
        var event: [String: Any] = ["type": "outcome", "request_id": request, "at": at, "task_id": task, "status": status,
                                    "answer": answer, "verified": verified]
        if let display { event["display_text"] = display }
        return event
    }
}

/// A daemon whose `chat.send` stream delivers a fixed list of events and then ends.
private final class ScriptedChatRPC: DaemonRPCTransport {
    let events: [[String: Any]]
    /// The text of each chat.send, as the daemon would receive it.
    private(set) var sent: [String] = []
    init(events: [[String: Any]]) { self.events = events }

    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
        ["session_id": "test", "capabilities": ["input_paths"], "running": [[String: Any]](),
         "approvals": [[String: Any]](), "results": [[String: Any]]()]
    }

    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        let events = method == "chat.send" ? self.events : []
        if method == "chat.send", let text = params["text"] as? String { sent.append(text) }
        return AsyncThrowingStream { continuation in
            for event in events { continuation.yield(event) }
            if method == "chat.send" { continuation.finish() }
        }
    }
}
