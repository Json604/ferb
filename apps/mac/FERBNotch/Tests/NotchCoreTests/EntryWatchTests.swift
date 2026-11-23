import XCTest
@testable import FERBNotch
@testable import NotchCore

/// The shared conversation as FERB.app follows it (ENG-10, ENG-14): FERB.app keeps writing its own turns from their
/// own streams, exactly as before; a turn another client sent (a phone; here a second client id) comes by
/// `chat.entries.watch` and is applied with the fold's per-entry mapping (`EntryFold`), live and after a relaunch.
@MainActor
final class EntryWatchTests: XCTestCase {
    private let phone = "phone-client-1"

    // MARK: the recorded scenarios

    /// The scenarios' transcripts (71 recorded, 1 invented, the phone's without a restart or another client's turn) are
    /// unchanged with the watch connected: the scenario's own entries,
    /// stamped with this install's client id, go through the watch (interleaved: each request's entries right after the
    /// frame that bound its turn, the rest at the end; then all again as a relaunch's first read) and change nothing.
    // decision: ENG-10
    // decision: APP-916
    func testOwnEntriesThroughTheWatchLeaveEveryRecordedTranscriptAsTheAppBuildsIt() async throws {
        var compared = 0
        var differences: [String] = []
        for scenario in try Self.golden() {
            let name = scenario["name"] as? String ?? "?"
            let timeline = scenario["timeline"] as? [[String: Any]] ?? []
            let rows = scenario["entries"] as? [[String: Any]] ?? []
            // a restart is a relaunch (EntryFoldTests), and a turn another client sent isn't this install's own
            if timeline.contains(where: { $0["do"] as? String == "restart"
                || ($0["frame"] as? [String: Any])?["client_id"] != nil }) { continue }
            for session in Set(timeline.compactMap { $0["session"] as? String }).sorted() {
                let steps = timeline.filter { $0["session"] as? String == session }
                let alone = EntryFoldTests.shape(await appArchive(steps, watched: false).conversation)
                let client = "this-install"
                let mine = rows.filter { $0["conversation_id"] as? String == session }
                    .map { Self.stamped($0, client: client) }
                    .sorted(by: { Self.int($0["seq"]) < Self.int($1["seq"]) })
                var fed = Set<String>()
                let model = await appArchive(steps, watched: true, client: client) { model in
                    // each request's entries as soon as its turn holds that request
                    for turn in model.conversation.turns {
                        guard let request = turn.requestID, !fed.contains(request) else { continue }
                        fed.insert(request)
                        for row in mine where row["request_id"] as? String == request {
                            if let entry = ChatEntry(row) { await model.applyEntryChange(entry, live: true) }
                        }
                    }
                }
                // what stayed for the end, live in the engine's order, then all again as a first read
                for row in mine where !fed.contains(row["request_id"] as? String ?? "") {
                    if let entry = ChatEntry(row) { await model.applyEntryChange(entry, live: true) }
                }
                _ = await model.applyCatchUp(ChatEntry.list(mine))
                let watched = EntryFoldTests.shape(model.conversation)
                compared += 1
                if watched != alone { differences.append("\(name) [\(session)]:\n  app:     \(alone)\n  watched: \(watched)") }
                model.disconnectDaemon()
            }
        }
        XCTAssertEqual(compared, 83)                // 71 recorded, 1 invented (ENG-18), 11 conversations for the phone (S6)
        XCTAssertTrue(differences.isEmpty, differences.joined(separator: "\n"))
    }

    /// Every recorded scenario whose entries all belong to message turns, sent by another client and read by the watch
    /// as a first read, gives the transcript the fold gives (the same per-entry mapping; the turns are marked sent
    /// elsewhere).
    // decision: ENG-10
    // decision: ENG-9
    // decision: APP-916
    func testAnotherClientsTurnsReadByTheWatchFoldAsTheEngineKeepsThem() async throws {
        var compared = 0
        var differences: [String] = []
        for scenario in try Self.golden() {
            let rows = scenario["entries"] as? [[String: Any]] ?? []
            let sessions = Set(rows.compactMap { $0["conversation_id"] as? String })
            // a scheduled run's answer names its run's request but no message started it: its turn is the result's
            let messages = Set(rows.filter { $0["kind"] as? String == "message" }.compactMap { $0["request_id"] as? String })
            guard sessions.count == 1, rows.allSatisfy({ messages.contains($0["request_id"] as? String ?? "") }) else { continue }
            let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults())
            model.entryWatch.supported = true
            let theirs = rows.map { Self.stamped($0, client: phone) }
            _ = await model.applyCatchUp(ChatEntry.list(theirs))
            XCTAssertTrue(model.conversation.turns.allSatisfy { $0.sentElsewhere == true })
            var unmarked = model.conversation
            for i in unmarked.turns.indices { unmarked.turns[i].sentElsewhere = nil }
            let folded = EntryFold.archive(ChatEntry.list(rows))
            compared += 1
            if EntryFoldTests.shape(unmarked) != EntryFoldTests.shape(folded) {
                differences.append("\(scenario["name"] ?? "?"):\n  fold:  \(EntryFoldTests.shape(folded))\n  watch: \(EntryFoldTests.shape(unmarked))")
            }
        }
        XCTAssertGreaterThanOrEqual(compared, 60, "65 of the 71 today")
        XCTAssertTrue(differences.isEmpty, differences.joined(separator: "\n"))
    }

    // MARK: a turn sent from another device

    /// A phone's turn appears live, in order (its message, its plan's card, its answer, a later answer), quietly; it is
    /// kept in the history; after a relaunch it is there, the watch resumes from the saved cursor and a revision it
    /// reads updates the later answer in place.
    // decision: ENG-10
    // decision: APP-914
    // decision: APP-571
    // decision: APP-916
    // decision: APP-918
    func testAPhonesTurnShowsLiveQuietlyAndComesBackAfterARelaunch() async throws {
        let preferences = scratchDefaults()
        let rpc = EntriesRPC()
        let model = connected(rpc, preferences)
        let sound = FakeSoundPlayer()
        model.feedback.player = sound
        model.collapse()
        await until { rpc.params(of: "chat.entries.watch").count == 1 && model.entryWatch.caughtUp }
        XCTAssertNil(rpc.params(of: "chat.entries.watch")[0]["epoch"], "no cursor: from now, no history read")

        let now = Date()
        rpc.push("chat.entries.watch",
                 change(entry("p1", pos: 10, seq: 11, kind: "message", request: "r-phone", client: phone,
                              body: ["text": "check the train times", "attachments": [], "hidden": []], at: now)))
        await until { model.conversation.turns.count == 1 }
        XCTAssertEqual(model.conversation.turns[0].request, "check the train times")
        XCTAssertEqual(model.conversation.turns[0].sentElsewhere, true)

        rpc.push("chat.entries.watch",
                 ["type": "live", "request_id": "r-phone",
                  "frame": ["type": "progress", "planning": true, "text": "Looking at the timetable", "at": iso(now)]])
        await until { model.conversation.turns[0].liveLine != nil }

        let card: [String: Any] = ["request_id": "r-phone", "task_id": "t-phone", "total": 2,
                                   "rows": [["part": "a", "label": "Morning trains", "state": "working", "seq": 1],
                                            ["part": "b", "label": "Evening trains", "state": "working", "seq": 1]]]
        rpc.push("chat.entries.watch",
                 change(entry("p2", pos: 11, seq: 12, kind: "plan", request: "r-phone", task: "t-phone", client: phone,
                              body: ["text": "I'll look up both.", "plan_line": "I'll look up both.", "lines": [],
                                     "card": card], at: now)))
        await until { model.conversation.turns[0].card != nil }
        XCTAssertEqual(model.conversation.turns[0].backendTaskID, "t-phone")
        XCTAssertNil(model.conversation.turns[0].liveLine, "the plan ends the live line")

        rpc.push("chat.entries.watch",
                 change(entry("p3", pos: 12, seq: 13, kind: "answer", request: "r-phone", task: "t-phone", client: phone,
                              placement: "inline", answers: "p1",
                              body: ["text": "Trains every 20 minutes.", "status": "done"], at: now)),
                 change(entry("p4", pos: 13, seq: 14, kind: "answer", request: "r-phone", task: "t-phone", client: phone,
                              placement: "inline", answers: "p1",
                              body: ["text": "The 8:10 is cancelled.", "status": "done", "follows": "p3"], at: now)))
        await until { model.conversation.replies.count == 1 }
        XCTAssertEqual(said(model), ["check the train times", "Trains every 20 minutes.", "later: The 8:10 is cancelled."])
        XCTAssertEqual(sound.played, [], "a phone's turn arrives quietly")
        XCTAssertFalse(model.expanded, "and never opens the notch")
        await until { (self.savedCursor(preferences)?["seq"] as? Int) == 14 }
        await until { rpc.history.rows.count >= 2 }                // the turn and its later answer, in the history
        // the mark goes in its own task (markEntries), so it may land just after the cursor is saved
        await until { rpc.params(of: "chat.entries.mark").contains { $0["delivered_seq"] as? Int == 14 } }
        model.disconnectDaemon()
        rpc.endAll()

        // a relaunch: the same preferences and history
        let again = EntriesRPC(history: rpc.history)
        again.script("chat.entries.watch", .frames([
            change(entry("p4", pos: 13, seq: 20, rev: 2, kind: "answer", request: "r-phone", task: "t-phone", client: phone,
                         placement: "inline", answers: "p1",
                         body: ["text": "The 8:10 is cancelled; take the 8:30.", "status": "done", "follows": "p3"],
                         at: now), catchup: true),
            ["type": "caught_up", "conversation": "S1", "epoch": "E1", "head": 20],
        ]))
        let relaunched = connected(again, preferences)
        await until { relaunched.entryWatch.caughtUp }
        let watch = try XCTUnwrap(again.params(of: "chat.entries.watch").first)
        XCTAssertEqual(watch["epoch"] as? String, "E1")
        XCTAssertEqual(watch["after_seq"] as? Int, 14)
        await until { relaunched.conversation.replies.first?.said?.answer == "The 8:10 is cancelled; take the 8:30." }
        XCTAssertEqual(said(relaunched), ["check the train times", "Trains every 20 minutes.",
                                          "later: The 8:10 is cancelled; take the 8:30."])
        XCTAssertEqual(relaunched.conversation.turns.first?.sentElsewhere, true)
        relaunched.disconnectDaemon()
        again.endAll()
    }

    /// A phone's turn's question arrives on the bus, binds to its task through the turn's plan, shows in its card row
    /// and is answered from the Mac as any question.
    // decision: ENG-10
    // decision: APP-300
    // decision: APP-917
    func testAPhonesQuestionIsAnsweredFromTheMac() async throws {
        let rpc = EntriesRPC()
        let model = connected(rpc, scratchDefaults())
        await until { model.entryWatch.caughtUp && rpc.methods().contains("logs.subscribe") }
        let now = Date()
        let card: [String: Any] = ["request_id": "r-phone", "task_id": "t-phone", "total": 1,
                                   "rows": [["part": "a", "label": "Book the table", "state": "working", "seq": 1,
                                             "step_task_id": "t-phone.1"]]]
        rpc.push("chat.entries.watch",
                 change(entry("p1", pos: 1, seq: 1, kind: "message", request: "r-phone", client: phone,
                              body: ["text": "book a table"], at: now)),
                 change(entry("p2", pos: 2, seq: 2, kind: "plan", request: "r-phone", task: "t-phone", client: phone,
                              body: ["text": "Booking.", "plan_line": "Booking.", "card": card], at: now)))
        await until { model.conversation.turns.first?.card != nil }
        rpc.push("logs.subscribe", ["type": "event", "topic": "approval.requested",
                                    "data": ["approval_id": "q1", "task_id": "t-phone.1", "part": "a",
                                             "kind": "owner_question", "reason": "Book for 7pm?",
                                             "options": ["Yes", "No"]]])
        await until { !model.pendingApprovals.isEmpty }
        let question = try XCTUnwrap(model.pendingApprovals.first)
        XCTAssertEqual(question.taskID, "t-phone", "bound to the phone's task, through its turn")
        XCTAssertEqual(model.conversation.rowPlacements(model.pendingApprovals)["q1"]?.turnID, model.conversation.turns[0].id)
        model.chooseApproval(question, index: 0)
        await until { rpc.methods().contains("chat.approve") }
        XCTAssertEqual(rpc.params(of: "chat.approve").first?["approval_id"] as? String, "q1")
        await until { model.pendingApprovals.isEmpty }
        model.disconnectDaemon()
        rpc.endAll()
    }

    /// A phone's turn goes where the engine placed it: before an own turn made after it (by pos once seen, by its ack's
    /// seq before that, or still waiting for its ack), else at the end.
    // decision: ENG-10
    // decision: APP-916
    func testAPhonesTurnAndOwnTurnsOrderByTheEnginesPlace() async throws {
        let rpc = EntriesRPC()
        rpc.held.insert("chat.send")
        rpc.script("chat.send",
                   .frames([["type": "ack", "request_id": "r-own-1", "seq": 5, "entry_id": "o1", "epoch": "E1",
                             "message": "first own"]]),
                   .frames([]))
        let model = connected(rpc, scratchDefaults())
        await until { model.entryWatch.caughtUp }
        model.send("first own")
        await until { model.conversation.turns.first?.requestID == "r-own-1" }
        // a phone message the engine made before this one (seq 4 < the ack's 5), arriving after its ack
        let now = Date()
        rpc.push("chat.entries.watch",
                 change(entry("p1", pos: 4, seq: 4, kind: "message", request: "r-phone-1", client: phone,
                              body: ["text": "phone before"], at: now)))
        await until { model.conversation.turns.count == 2 }
        XCTAssertEqual(model.conversation.turns.map(\.request), ["phone before", "first own"])
        // the own message's entry: its place is known from now on
        rpc.push("chat.entries.watch",
                 change(entry("o1", pos: 5, seq: 5, kind: "message", request: "r-own-1", client: model.clientID,
                              body: ["text": "first own"], at: now)))
        // a second own message still waiting for its ack, and a phone message after the first own one
        model.send("second own")
        await until { model.conversation.turns.count == 3 }
        rpc.push("chat.entries.watch",
                 change(entry("p2", pos: 6, seq: 6, kind: "message", request: "r-phone-2", client: phone,
                              body: ["text": "phone after"], at: now)))
        await until { model.conversation.turns.count == 4 }
        XCTAssertEqual(model.conversation.turns.map(\.request), ["phone before", "first own", "phone after", "second own"])
        let dates = model.conversation.turns.map(\.createdAt)
        XCTAssertEqual(dates, dates.sorted(), "dated in that order, so the history reads them back the same way")
        model.disconnectDaemon()
        rpc.endAll()
    }

    /// The same phone turn arriving both by the bus (`task.result`) and by the watch, in either order, and changes
    /// delivered twice (a relaunch's re-read, a reset), never make a second turn or a second answer.
    // decision: ENG-10
    // decision: APP-914
    // decision: APP-917
    func testNoDuplicateFromTheBusTheWatchOrARepeatedDelivery() async throws {
        let rpc = EntriesRPC()
        let preferences = scratchDefaults()
        let model = connected(rpc, preferences)
        await until { model.entryWatch.caughtUp && rpc.methods().contains("logs.subscribe") }
        let now = Date()
        // the bus first: its result is filed for the request, then the message fills that very turn
        rpc.push("logs.subscribe", ["type": "event", "topic": "task.result", "at": iso(now),
                                    "data": ["task_id": "t-a", "request_id": "r-a", "status": "done",
                                             "answer": "Done A.", "goal": "Do A"]])
        await until { model.conversation.turns.count == 1 }
        let filed = model.conversation.turns[0].id
        let a = [entry("a1", pos: 1, seq: 1, kind: "message", request: "r-a", task: "t-a", client: phone,
                       body: ["text": "do A"], at: now),
                 entry("a2", pos: 2, seq: 2, kind: "plan", request: "r-a", task: "t-a", client: phone,
                       body: ["text": "Doing A."], at: now),
                 entry("a3", pos: 3, seq: 3, kind: "answer", request: "r-a", task: "t-a", client: phone,
                       placement: "inline", answers: "a1", body: ["text": "Done A.", "status": "done"], at: now)]
        for row in a { rpc.push("chat.entries.watch", change(row)) }
        await until { model.conversation.turns.first?.plan == "Doing A." }
        XCTAssertEqual(model.conversation.turns.map(\.id), [filed])
        XCTAssertEqual(model.conversation.turns[0].request, "do A")
        XCTAssertNil(model.conversation.turns[0].taskGoal)

        // the watch first: the result later writes nothing of the turn (its answer is the entry's)
        let b = [entry("b1", pos: 4, seq: 4, kind: "message", request: "r-b", client: phone, body: ["text": "do B"], at: now),
                 entry("b2", pos: 5, seq: 5, kind: "plan", request: "r-b", task: "t-b", client: phone,
                       body: ["text": "Doing B."], at: now),
                 entry("b3", pos: 6, seq: 6, kind: "answer", request: "r-b", task: "t-b", client: phone,
                       placement: "inline", answers: "b1", body: ["text": "Planner says B.", "status": "done"], at: now)]
        for row in b { rpc.push("chat.entries.watch", change(row)) }
        await until { model.conversation.turns.last?.answer == "Planner says B." }
        rpc.push("logs.subscribe", ["type": "event", "topic": "task.result", "at": iso(now),
                                    "data": ["task_id": "t-b", "request_id": "r-b", "status": "done",
                                             "answer": "Task says B.", "goal": "Do B"]])
        await until { !model.runningTasks.keys.contains("t-b") && model.knownCompletedTasks.contains("t-b") }
        XCTAssertEqual(model.conversation.turns.count, 2)
        XCTAssertTrue(model.conversation.replies.isEmpty, "the result never adds a later answer of its own")
        XCTAssertEqual(model.conversation.turns[1].answer, "Planner says B.")

        // delivered twice: nothing changes
        let before = model.conversation
        for row in a + b { rpc.push("chat.entries.watch", change(row)) }
        await until { (self.savedCursor(preferences)?["seq"] as? Int) == 6 }
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(EntryFoldTests.shape(model.conversation), EntryFoldTests.shape(before))

        // a reset (another store): the cursor goes, the watch reads again from now, and a change seen again finds its
        // turn by request and entry id
        rpc.script("chat.entries.watch", .frames([["type": "reset_required", "conversation": "S1", "epoch": "E2", "head": 3]]),
                   .frames([["type": "caught_up", "conversation": "S1", "epoch": "E2", "head": 3]] + (a + b).map { change($0) }))
        rpc.end("chat.entries.watch")
        await until { rpc.params(of: "chat.entries.watch").count == 3 && model.entryWatch.caughtUp }
        XCTAssertNil(rpc.params(of: "chat.entries.watch")[2]["epoch"], "watched again from now")
        await until { self.savedCursor(preferences)?["epoch"] as? String == "E2" }
        XCTAssertEqual(EntryFoldTests.shape(model.conversation), EntryFoldTests.shape(before))
        model.disconnectDaemon()
        rpc.endAll()
    }

    /// An older daemon (no "entries"): no `chat.entries.*` call, `chat.send`'s params as today, no resend.
    // decision: ENG-10
    // decision: ENG-14
    // decision: APP-916
    func testAnOlderDaemonSeesTheAppAsBefore() async throws {
        let rpc = EntriesRPC(capabilities: ["input_paths"])
        rpc.script("chat.send", .fail(DaemonRPCError.disconnected))
        let model = connected(rpc, scratchDefaults())
        await until { rpc.methods().contains("logs.subscribe") }
        model.send("hello")
        await until { model.conversation.turns.first?.status == "interrupted" }
        XCTAssertEqual(rpc.params(of: "chat.send").count, 1, "no resend")
        XCTAssertEqual(Set(rpc.params(of: "chat.send")[0].keys), ["session_id", "text"])
        XCTAssertFalse(rpc.methods().contains { $0.hasPrefix("chat.entries.") })
        XCTAssertNil(model.preferences.string(forKey: NotchModel.clientIDKey))
        model.disconnectDaemon()
        rpc.endAll()
    }

    /// A send whose link dropped before any frame is sent once more with the same ids; the engine had it (`repeat`), so
    /// this run fills that own turn from the entries, those already seen first, and the turn is not cut off.
    // decision: ENG-14
    // decision: APP-918
    func testAResendWithTheSameIdsAdoptsTheTurnTheEngineAlreadyHad() async throws {
        let rpc = EntriesRPC()
        rpc.script("chat.send", .fail(DaemonRPCError.disconnected),
                   .frames([["type": "ack", "request_id": "r-own", "seq": 2, "entry_id": "o1", "epoch": "E1",
                             "submission_id": "x", "repeat": true, "message": "plan my day"]]))
        let model = connected(rpc, scratchDefaults())
        await until { model.entryWatch.caughtUp }
        let now = Date()
        // the first send reached the engine: its message came by the watch, with this install's id, before the ack
        rpc.push("chat.entries.watch",
                 change(entry("o1", pos: 2, seq: 2, kind: "message", request: "r-own", client: model.clientID,
                              body: ["text": "plan my day"], at: now)))
        try await Task.sleep(for: .milliseconds(50))
        model.send("plan my day")
        await until { model.entryWatch.adopted.contains("r-own") }
        let sends = rpc.params(of: "chat.send")
        XCTAssertEqual(sends.count, 2)
        XCTAssertEqual(sends[0]["client_id"] as? String, model.clientID)
        XCTAssertNotNil(sends[0]["submission_id"] as? String)
        XCTAssertEqual(sends[0]["submission_id"] as? String, sends[1]["submission_id"] as? String)
        XCTAssertEqual(sends[0]["client_id"] as? String, sends[1]["client_id"] as? String)
        await until { model.submissions.isEmpty }
        XCTAssertEqual(model.conversation.turns.count, 1)
        XCTAssertEqual(model.conversation.turns[0].status, "working", "not cut off when its stream ended")
        rpc.push("chat.entries.watch",
                 change(entry("o2", pos: 3, seq: 3, kind: "plan", request: "r-own", task: "t-own", client: model.clientID,
                              body: ["text": "Planning your day."], at: now)),
                 change(entry("o3", pos: 4, seq: 4, kind: "answer", request: "r-own", task: "t-own",
                              client: model.clientID, placement: "inline", answers: "o1",
                              body: ["text": "Your day is planned.", "status": "done"], at: now)))
        await until { model.conversation.turns.first?.answer == "Your day is planned." }
        XCTAssertEqual(model.conversation.turns.count, 1)
        XCTAssertEqual(model.conversation.turns[0].plan, "Planning your day.")
        XCTAssertNil(model.conversation.turns[0].sentElsewhere, "still this app's own turn")
        model.disconnectDaemon()
        rpc.endAll()
    }

    /// An own turn's entries are left to its own stream: nothing is applied twice.
    // decision: ENG-10
    // decision: APP-916
    func testOwnEntriesAreLeftToTheirOwnStream() async throws {
        let rpc = EntriesRPC()
        rpc.script("chat.send", .frames([
            ["type": "ack", "request_id": "r-own", "seq": 1, "message": "hi"],
            ["type": "reply", "request_id": "r-own", "text": "Hello.", "status": "done"],
        ]))
        let model = connected(rpc, scratchDefaults())
        await until { model.entryWatch.caughtUp }
        model.send("hi")
        await until { model.conversation.turns.first?.answer == "Hello." }
        let before = EntryFoldTests.shape(model.conversation)
        let now = Date()
        rpc.push("chat.entries.watch",
                 change(entry("o1", pos: 1, seq: 1, kind: "message", request: "r-own", client: model.clientID,
                              body: ["text": "hi"], at: now)),
                 change(entry("o2", pos: 2, seq: 2, kind: "answer", request: "r-own", client: model.clientID,
                              placement: "inline", answers: "o1", body: ["text": "Hello again.", "status": "done"],
                              at: now)))
        await until { model.entryWatch.positions["r-own"] == 1 }
        try await Task.sleep(for: .milliseconds(50))
        XCTAssertEqual(EntryFoldTests.shape(model.conversation), before)
        model.disconnectDaemon()
        rpc.endAll()
    }

    /// The marks: what this install saved, after each read, and the newest entry seen while today's chat shows; sent
    /// only when they move.
    // decision: ENG-14
    // decision: APP-918
    func testMarksAreSentWhenTheyMove() async throws {
        let rpc = EntriesRPC()
        let model = connected(rpc, scratchDefaults())
        await until { model.entryWatch.caughtUp }
        await until { rpc.params(of: "chat.entries.mark").count == 1 }
        XCTAssertEqual(rpc.params(of: "chat.entries.mark")[0]["delivered_seq"] as? Int, 0)
        XCTAssertEqual(rpc.params(of: "chat.entries.mark")[0]["client_id"] as? String, model.clientID)
        model.expand()
        rpc.push("chat.entries.watch",
                 change(entry("p1", pos: 7, seq: 9, kind: "message", request: "r-phone", client: phone,
                              body: ["text": "hello from the phone"], at: Date())))
        await until { rpc.params(of: "chat.entries.mark").contains { $0["delivered_seq"] as? Int == 9 } }
        let mark = try XCTUnwrap(rpc.params(of: "chat.entries.mark").last)
        XCTAssertEqual(mark["read_pos"] as? Int, 7)
        let count = rpc.params(of: "chat.entries.mark").count
        model.markEntries(delivered: 9)
        XCTAssertEqual(rpc.params(of: "chat.entries.mark").count, count, "nothing moved: nothing sent")
        model.disconnectDaemon()
        rpc.endAll()
    }

    // MARK: helpers

    private func connected(_ rpc: EntriesRPC, _ preferences: UserDefaults) -> NotchModel {
        let model = NotchModel(rpc: rpc, preferences: preferences)
        model.entryWatch.retryWait = .milliseconds(10)
        model.entryWatch.catchUpWait = .milliseconds(500)
        model.connectDaemon()
        addTeardownBlock { @MainActor in model.disconnectDaemon(); rpc.endAll() }
        return model
    }

    /// Returns the moment `done` holds: the bound only ends a failing run, so it is wide (a loaded machine, TEST-508).
    private func until(_ done: () -> Bool, timeout: TimeInterval = 30, file: StaticString = #filePath, line: UInt = #line) async {
        let end = Date().addingTimeInterval(timeout)
        while !done() {
            if Date() > end { XCTFail("timed out", file: file, line: line); return }
            try? await Task.sleep(for: .milliseconds(5))
        }
    }

    private func savedCursor(_ preferences: UserDefaults) -> [String: Any]? {
        preferences.dictionary(forKey: NotchModel.entryCursorKey)
    }

    private func iso(_ date: Date) -> String { Date.ISO8601FormatStyle(includingFractionalSeconds: true).format(date) }

    private func change(_ entry: [String: Any], catchup: Bool = false) -> [String: Any] {
        var out: [String: Any] = ["type": "change", "entry": entry]
        if catchup { out["catchup"] = true }
        return out
    }

    private func entry(_ id: String, pos: Int, seq: Int, rev: Int = 1, kind: String, request: String?,
                       task: String? = nil, client: String?, placement: String? = nil, answers: String? = nil,
                       body: [String: Any], at: Date) -> [String: Any] {
        ["id": id, "conversation_id": "S1", "pos": pos, "seq": seq, "rev": rev, "kind": kind, "state": "done",
         "open": false, "request_id": request ?? NSNull(), "task_id": task ?? NSNull(), "approval_id": NSNull(),
         "ref": NSNull(), "answers": answers ?? NSNull(), "placement": placement ?? NSNull(), "origin": NSNull(),
         "client_id": client ?? NSNull(), "submission_id": NSNull(), "journal_id": NSNull(), "created_at": iso(at),
         "updated_at": iso(at), "day": HistoryEntry.day(at), "body": body, "deleted": false]
    }

    /// The chat top to bottom as words: a turn's message and answer, a later answer.
    private func said(_ model: NotchModel) -> [String] {
        model.conversation.transcript().flatMap { entry -> [String] in
            switch entry {
            case let .turn(turn, _): return [turn.request, turn.answer].filter { !$0.isEmpty }
            case let .reply(reply, shown): return [(reply.said == nil ? "below: " : "later: ") + shown.answer]
            case let .notice(notice): return ["notice: " + notice.text]
            }
        }
    }

    private static func stamped(_ row: [String: Any], client: String) -> [String: Any] {
        var out = row
        out["client_id"] = row["request_id"] is String ? client : NSNull()
        return out
    }

    private static func int(_ value: Any?) -> Int { (value as? NSNumber)?.intValue ?? 0 }

    /// EntryFoldTests' app path (the scenario's timeline through FERB.app's reducer), with the shared conversation on
    /// (`watched`: the daemon has entries, so own turns are this install's and results file their request).
    private func appArchive(_ timeline: [[String: Any]], watched: Bool, client: String = "",
                            after: ((NotchModel) async -> Void)? = nil) async -> NotchModel {
        let preferences = scratchDefaults()
        if watched { preferences.set(client, forKey: NotchModel.clientIDKey) }
        let model = NotchModel(rpc: ScriptedRPC(), preferences: preferences)
        model.entryWatch.supported = watched
        var local: [Int: UUID] = [:]
        for item in timeline {
            let stream = (item["stream"] as? NSNumber)?.intValue ?? -1
            switch item["do"] as? String {
            case "send":
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
            case "legacy":
                // a row the app saved in chat_history, as its history gives it back (EntryFoldTests.appModel)
                if let stored = HistoryEntry.read(["kind": item["kind"] ?? "", "body": item["body"] ?? [:]]) {
                    model.conversation.absorb([stored])
                }
            case "delete":
                model.removeTurns(requestIDs: [item["request_id"] as? String ?? ""])
            case "fresh":
                _ = model.conversation.addNotice(id: "conversation:" + (item["session"] as? String ?? ""),
                                                 text: StoredData.freshMarker, tag: nil,
                                                 at: LiveProgress.time(item["at"]) ?? Date())
            default:
                XCTFail("unknown timeline step \(item["do"] ?? "")")
            }
            await after?(model)
        }
        return model
    }

    private static func golden() throws -> [[String: Any]] {
        var repo = URL(fileURLWithPath: #filePath)
        for _ in 0..<6 { repo = repo.deletingLastPathComponent() }
        let url = repo.appendingPathComponent("engine/fixtures/chat-entries/scenarios.json")
        let object = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any]
        return object?["scenarios"] as? [[String: Any]] ?? []
    }
}
