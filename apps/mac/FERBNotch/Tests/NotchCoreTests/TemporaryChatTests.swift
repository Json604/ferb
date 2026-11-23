import XCTest
@testable import FERBNotch
@testable import NotchCore

/// The temporary chat (owner 2026-11-23: "temporary chat - where there are all the tools n shit BUT no prior context
/// given"; "the output,messages are all in ememory"; "it shodln be the same small icon that turned it on taht turns it
/// off"), driven through the real model against a daemon that holds it in memory as the contract says: the icon turns
/// it on and off, the chat then shows only it, the composer sends into it, a relaunch keeps it, and nothing of it ever
/// reaches the saved chat or its history.
@MainActor
final class TemporaryChatTests: XCTestCase {
    /// A daemon with a temporary chat (`chat.temporary.get`/`.set`, `chat.send` with `fresh`, chat.status's
    /// `temporary`, the bus's `chat.temporary`), the chat history kept by a `HistoryRPC`, every call recorded.
    final class TemporaryRPC: DaemonRPCTransport, @unchecked Sendable {
        private let lock = NSLock()
        private var recorded: [(String, [String: Any])] = []
        let history: HistoryRPC
        var on = false
        var id = "t1"
        var rev = 0
        var entries: [[String: Any]] = []
        var refuseSet = false
        var approvals: [[String: Any]] = []
        var running: [[String: Any]] = []
        /// What a fresh chat.send streams, after its message entry is kept.
        var reply = "Fresh answer"
        private var subscribers: [AsyncThrowingStream<[String: Any], Error>.Continuation] = []

        init(history: HistoryRPC = HistoryRPC()) { self.history = history }

        var calls: [(String, [String: Any])] { lock.withLock { recorded } }
        func params(of method: String) -> [[String: Any]] { calls.filter { $0.0 == method }.map(\.1) }

        private var reading: [String: Any] {
            guard on else { return ["on": false] }
            return ["on": true, "id": id, "conversation": "temporary:" + id, "started_at": "2026-11-23T10:00:00Z",
                    "rev": rev, "entries": entries]
        }

        func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
            lock.withLock { recorded.append((method, params)) }
            switch method {
            case "chat.attach": return ["session_id": "S1", "capabilities": ["input_paths"]]
            case "chat.status":
                var status: [String: Any] = ["results": [], "approvals": approvals, "loops": [], "notices": [],
                                             "running": running, "paused": false, "stopped": false]
                if on { status["temporary"] = ["id": id, "rev": rev, "started_at": "2026-11-23T10:00:00Z"] }
                return status
            case "chat.temporary.get": return lock.withLock { reading }
            case "chat.temporary.set":
                if refuseSet { throw DaemonRPCError.unavailable }
                return lock.withLock {
                    let want = params["on"] as? Bool ?? false
                    if want && !on { on = true; rev = 1; entries = [] }
                    if !want && on { on = false; entries = [] }
                    return reading
                }
            default:
                if method.hasPrefix("chat.history.") { return try await history.call(method, params: params) }
                return ["ok": true]
            }
        }

        func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
            lock.withLock { recorded.append((method, params)) }
            return AsyncThrowingStream { continuation in
                switch method {
                case "logs.subscribe":
                    self.lock.withLock { self.subscribers.append(continuation) }
                case "chat.send" where params["fresh"] as? Bool == true:
                    let text = params["text"] as? String ?? ""
                    let request = "temporary-r\(self.entries.count + 1)"
                    self.keep("message", request: request, body: ["text": text, "attachments": [], "hidden": []])
                    continuation.yield(["type": "ack", "request_id": request, "fresh": true,
                                        "conversation": "temporary:" + self.id])
                    continuation.yield(["type": "reply_delta", "request_id": request, "text": "Fresh"])
                    self.keep("answer", request: request, body: ["text": self.reply, "status": "done"])
                    continuation.yield(["type": "reply", "request_id": request, "text": self.reply, "status": "done"])
                    continuation.finish()
                default:
                    continuation.finish()
                }
            }
        }

        /// Keeps an entry of the temporary chat (on, if it wasn't) and tells the bus.
        func keep(_ kind: String, request: String, task: String? = nil, body: [String: Any]) {
            lock.withLock {
                if !on { on = true; rev = 1 }
                let pos = entries.count + 1
                var entry: [String: Any] = ["id": "e\(pos)", "conversation_id": "temporary:" + id, "pos": pos,
                                            "seq": pos, "rev": 1, "kind": kind, "state": "done", "open": false,
                                            "request_id": request, "created_at": "2026-11-23T10:00:0\(min(pos, 9))Z",
                                            "updated_at": "2026-11-23T10:00:00Z", "day": "2026-11-23", "body": body]
                if let task { entry["task_id"] = task }
                entries.append(entry)
                rev += 1
            }
            push(["topic": "chat.temporary", "data": ["on": true, "id": id, "rev": rev]])
        }

        func push(_ event: [String: Any]) {
            for subscriber in lock.withLock({ subscribers }) { subscriber.yield(event) }
        }

        func endAll() {
            for subscriber in lock.withLock({ subscribers }) { subscriber.finish() }
            lock.withLock { subscribers = [] }
        }
    }

    // MARK: helpers

    private func connected(_ rpc: TemporaryRPC, _ name: String) throws -> NotchModel {
        let defaults = scratchDefaults(name)
        defaults.set(try JSONEncoder().encode(HistoryImport(at: Date(), entries: 0, archiveSHA256: "")),
                     forKey: NotchModel.historyImportKey)
        let model = NotchModel(rpc: rpc, preferences: defaults)
        XCTAssertEqual(model.chatStore.store, .daemon)
        model.connectDaemon()
        addTeardownBlock { @MainActor in model.disconnectDaemon(); rpc.endAll() }
        return model
    }

    private func until(_ done: () -> Bool, timeout: TimeInterval = 3, file: StaticString = #filePath,
                       line: UInt = #line) async {
        let end = Date().addingTimeInterval(timeout)
        while !done() {
            if Date() > end { XCTFail("timed out", file: file, line: line); return }
            try? await Task.sleep(for: .milliseconds(5))
        }
    }

    /// The chat as drawn, top to bottom, as words: each turn's message and answer.
    private func shown(_ model: NotchModel) -> [String] {
        model.visibleEntries.flatMap { entry -> [String] in
            if case let .turn(turn, _) = entry { return [turn.request, turn.answer].filter { !$0.isEmpty } }
            return []
        }
    }

    /// A saved turn of the normal chat, written to FERB's database as every turn is.
    private func savedTurn(_ model: NotchModel, _ rpc: TemporaryRPC) async {
        let id = UUID()
        model.conversation.begin(id: id, request: "Plan my week", sources: [])
        model.conversation.finish(id: id, answer: "Here is your week.", status: "done", artifacts: [])
        model.saveConversation()
        await until { rpc.history.held(id) != nil }
    }

    /// Every text the chat history was ever given.
    private func historyText(_ rpc: TemporaryRPC) -> String {
        rpc.history.calls.filter { $0.0.hasPrefix("chat.history.put") || $0.0 == "chat.history.import" }
            .map { "\($0.1)" }.joined(separator: "\n")
    }

    // MARK: the icon

    // The icon turns it on: the banner shows and the chat shows only the temporary conversation; the icon's label
    // says it ends it. The icon again turns it off: the saved chat comes back exactly as it was.
    // decision: APP-1122
    // decision: ORCH-700
    func testTheIconTurnsItOnShowingOnlyTheTemporaryChatAndTheSameIconTurnsItOff() async throws {
        let rpc = TemporaryRPC()
        let model = try connected(rpc, "TempToggle")
        await until { rpc.params(of: "logs.subscribe").count == 1 }
        await savedTurn(model, rpc)
        let saved = model.conversation
        XCTAssertEqual(shown(model), ["Plan my week", "Here is your week."])
        XCTAssertFalse(model.inTemporaryChat)

        model.toggleTemporaryChat()
        await until { model.inTemporaryChat }
        XCTAssertEqual(rpc.params(of: "chat.temporary.set").last?["on"] as? Bool, true)
        XCTAssertTrue(shown(model).isEmpty, "the saved chat isn't shown in the temporary chat")
        XCTAssertFalse(model.showsTranscript, "empty: the line saying how it ends shows in the greeting's place")

        // another device sends into it: its entries come by the bus and only they show
        rpc.keep("message", request: "temporary-p1", body: ["text": "What's 2+2?", "attachments": [], "hidden": []])
        rpc.keep("answer", request: "temporary-p1", body: ["text": "4", "status": "done"])
        await until { self.shown(model) == ["What's 2+2?", "4"] }
        XCTAssertEqual(model.conversation, saved, "the saved chat is untouched")

        model.toggleTemporaryChat()
        await until { !model.inTemporaryChat }
        XCTAssertEqual(rpc.params(of: "chat.temporary.set").last?["on"] as? Bool, false)
        XCTAssertEqual(model.conversation, saved)
        XCTAssertEqual(shown(model), ["Plan my week", "Here is your week."], "the saved chat comes back as it was")
        XCTAssertFalse(historyText(rpc).contains("2+2"), "nothing temporary reached the chat history")
    }

    // A tap FERB couldn't carry out says so and changes nothing.
    // decision: APP-1122
    func testAFailedTapSaysSoAndChangesNothing() async throws {
        let rpc = TemporaryRPC()
        rpc.refuseSet = true
        let model = try connected(rpc, "TempRefused")
        await until { rpc.params(of: "logs.subscribe").count == 1 }
        model.toggleTemporaryChat()
        await until { model.promptError != nil }
        XCTAssertEqual(model.promptError, NotchModel.temporaryChatStartFailed)
        XCTAssertFalse(model.inTemporaryChat)
        XCTAssertFalse(model.temporaryToggling)
    }

    // MARK: the composer

    // The composer's message goes with `fresh: true`; its words and reply show in the temporary chat, and none of it
    // reaches the saved chat, its history or its turns.
    // decision: APP-1122
    // decision: ORCH-699
    func testTheComposerSendsFreshAndNothingOfItIsSaved() async throws {
        let rpc = TemporaryRPC()
        let model = try connected(rpc, "TempSend")
        await until { rpc.params(of: "logs.subscribe").count == 1 }
        await savedTurn(model, rpc)
        let saved = model.conversation
        let puts = rpc.history.count("chat.history.put")
        model.toggleTemporaryChat()
        await until { model.inTemporaryChat }

        model.session.draft = "Draft a haiku about rain"
        model.startWork()
        await until { self.shown(model) == ["Draft a haiku about rain", "Fresh answer"] }
        let send = try XCTUnwrap(rpc.params(of: "chat.send").last)
        XCTAssertEqual(send["fresh"] as? Bool, true)
        XCTAssertEqual(send["text"] as? String, "Draft a haiku about rain")
        XCTAssertTrue(model.session.draft.isEmpty, "the composer clears on sending")
        await until { model.temporaryChat?.pending.isEmpty == true }

        try await Task.sleep(for: .milliseconds(400))        // longer than the history's write debounce
        XCTAssertEqual(model.conversation, saved, "no turn of the temporary chat in the saved chat")
        XCTAssertEqual(rpc.history.count("chat.history.put"), puts, "no chat.history.put while it was on")
        XCTAssertFalse(historyText(rpc).contains("haiku"))
        XCTAssertFalse(historyText(rpc).contains("Fresh answer"))
        XCTAssertNil(model.preferences.data(forKey: "FERBConversationArchive"), "no archive written")
    }

    // MARK: relaunch and the daemon's word

    // Relaunching FERB.app doesn't end it: the new app reads it from the daemon and shows it.
    // decision: ORCH-700
    func testARelaunchKeepsIt() async throws {
        let rpc = TemporaryRPC()
        rpc.keep("message", request: "temporary-p1", body: ["text": "Still here?", "attachments": [], "hidden": []])
        rpc.keep("answer", request: "temporary-p1", body: ["text": "Yes", "status": "done"])
        let model = try connected(rpc, "TempRelaunch")
        await until { model.inTemporaryChat }
        XCTAssertEqual(shown(model), ["Still here?", "Yes"])
        XCTAssertTrue(rpc.params(of: "chat.temporary.set").isEmpty, "only the icon turns it on or off")
    }

    // The daemon saying it is off (its bus event; its status without `temporary`, after a restart) drops the copy here
    // and the saved chat shows.
    // decision: ORCH-700
    func testTheDaemonSayingOffDropsIt() async throws {
        let rpc = TemporaryRPC()
        rpc.keep("message", request: "temporary-p1", body: ["text": "Hi", "attachments": [], "hidden": []])
        let model = try connected(rpc, "TempOff")
        await until { model.inTemporaryChat && rpc.params(of: "logs.subscribe").count == 1 }
        rpc.lockedOff()
        rpc.push(["topic": "chat.temporary", "data": ["on": false, "id": "t1", "rev": 9]])
        await until { !model.inTemporaryChat }

        model.applyTemporaryChat(["on": true, "id": "t1", "rev": 3, "entries": []])
        XCTAssertTrue(model.inTemporaryChat)
        await model.refreshDaemonStatus()                    // a restarted daemon's status has no `temporary`
        XCTAssertFalse(model.inTemporaryChat)
    }

    // MARK: questions, results and entries of the temporary chat

    // Its question shows only in the temporary chat; its running task and its result never reach the saved chat, and an
    // entry of a temporary conversation is never written by the entry watch.
    // decision: APP-1122
    func testItsQuestionsTasksAndEntriesStayOutOfTheSavedChat() async throws {
        let rpc = TemporaryRPC()
        rpc.keep("message", request: "temporary-p1", body: ["text": "Book it", "attachments": [], "hidden": []])
        rpc.keep("plan", request: "temporary-p1", task: "task-temp", body: ["text": "I'll book it."])
        rpc.approvals = [["approval_id": "ap-temp", "task_id": "task-temp", "request_id": "temporary-p1",
                          "temporary": true, "reason": "Pay 10?", "options": ["Yes", "No"]],
                         ["approval_id": "ap-saved", "task_id": "task-saved", "reason": "Send it?",
                          "options": ["Yes", "No"]]]
        rpc.running = [["task_id": "task-temp", "goal": "Book it", "temporary": true],
                       ["task_id": "task-saved", "goal": "Send the mail"]]
        let model = try connected(rpc, "TempQuestions")
        await until { model.inTemporaryChat && model.pendingApprovals.count == 2 }
        let saved = model.conversation
        XCTAssertEqual(Set(model.inlineQuestions.map(\.id)), ["ap-temp", "ap-saved"])
        XCTAssertEqual(Array(model.runningTasks.keys), ["task-saved"], "its task isn't the saved chat's work")

        model.receiveOutcome(["task_id": "task-temp", "request_id": "temporary-p1", "status": "done",
                              "answer": "Booked"])
        XCTAssertEqual(model.conversation, saved, "its result isn't filed in the saved chat")

        let entry = try XCTUnwrap(ChatEntry(["id": "x1", "conversation_id": "temporary:t1", "pos": 1, "seq": 1,
                                             "kind": "message", "request_id": "temporary-p9", "client_id": "phone",
                                             "body": ["text": "From the phone"]]))
        model.entryWatch.supported = true
        let wrote = await model.applyEntryChange(entry, live: true)
        XCTAssertFalse(wrote)
        XCTAssertEqual(model.conversation, saved)

        // off (its status still lists the question for a moment): the saved chat never shows it
        model.endTemporaryChat()
        XCTAssertEqual(model.inlineQuestions.map(\.id), ["ap-saved"])
        XCTAssertFalse(historyText(rpc).contains("Book it"))
    }
}

private extension TemporaryChatTests.TemporaryRPC {
    /// The daemon ended it (another device's icon).
    func lockedOff() { on = false; entries = [] }
}
