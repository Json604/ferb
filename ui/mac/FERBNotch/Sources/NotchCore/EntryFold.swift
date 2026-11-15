import Foundation

// Stage 2 of the Rust engine (ENG-8): the engine keeps the chat as entries (`ferb_core::chat_entry`), made where it
// sends today's `chat.send` frames and bus events, so every client reads the same conversation. This file reads those
// entries and folds them into the archive FERB.app draws today (`ConversationArchive`). Not called by the app yet:
// a later step moves the app to the engine's entries; until then `EntryFoldTests` proves the fold gives the transcript
// the app's own reducer builds from the same frames.

/// One entry as the engine sends it (`{id, conversation_id, pos, seq, rev, kind, state, open, …, body}`).
public struct ChatEntry {
    public let id: String
    public let conversationID: String
    public let pos: Int
    public let seq: Int
    public let rev: Int
    /// message, plan, answer, question, notice, post, error or legacy.
    public let kind: String
    public let state: String
    public let open: Bool
    public let requestID: String?
    public let taskID: String?
    public let approvalID: String?
    public let ref: String?
    /// An answer's message entry.
    public let answers: String?
    /// inline or below (APP-580).
    public let placement: String?
    public let origin: String?
    public let createdAt: String
    public let updatedAt: String
    public let day: String
    public let body: [String: Any]
    public let deleted: Bool

    /// Read tolerantly (APP-801): an entry without its id, kind or order is left out.
    public init?(_ value: Any?) {
        guard let value = value as? [String: Any], let id = value["id"] as? String, !id.isEmpty,
              let kind = value["kind"] as? String, let pos = (value["pos"] as? NSNumber)?.intValue else { return nil }
        func words(_ key: String) -> String? { value[key] as? String }
        self.id = id
        self.kind = kind
        self.pos = pos
        conversationID = words("conversation_id") ?? ""
        seq = (value["seq"] as? NSNumber)?.intValue ?? 0
        rev = (value["rev"] as? NSNumber)?.intValue ?? 1
        state = words("state") ?? ""
        open = value["open"] as? Bool ?? false
        requestID = words("request_id")
        taskID = words("task_id")
        approvalID = words("approval_id")
        ref = words("ref")
        answers = words("answers")
        placement = words("placement")
        origin = words("origin")
        createdAt = words("created_at") ?? ""
        updatedAt = words("updated_at") ?? ""
        day = words("day") ?? ""
        body = value["body"] as? [String: Any] ?? [:]
        deleted = value["deleted"] as? Bool ?? false
    }

    /// The entries of a JSON list (each revision of an entry may be among them).
    public static func list(_ value: Any?) -> [ChatEntry] {
        (value as? [Any] ?? []).compactMap(ChatEntry.init)
    }
}

public enum EntryFold {
    /// The archive the entries make, as FERB.app holds it: a message with its plan and answer is a turn; an answer
    /// placed below is a late reply quoting its turn; an answer that `follows` the message's answer is a later answer
    /// in its own words (APP-914); an answer with no message (a resumed or scheduled task) is
    /// FERB's own turn; notices and posts are notices; an imported entry is read as the app saved it. Questions are not
    /// in the archive (the app holds open ones apart, `pendingApprovals`). Each entry's newest revision counts; a
    /// deleted one is left out.
    public static func archive(_ entries: [ChatEntry]) -> ConversationArchive {
        var newest: [String: ChatEntry] = [:]
        for entry in entries where (newest[entry.id]?.rev ?? 0) < entry.rev { newest[entry.id] = entry }
        let ordered = newest.values.filter { !$0.deleted }.sorted { $0.pos < $1.pos }

        var archive = ConversationArchive()
        var turnOf: [String: Int] = [:]          // a message entry, or a recovered answer, → its turn
        var byRequest: [String: Int] = [:]
        var shown: [Int: ChatEntry] = [:]        // the answer each turn shows
        // The archive orders late replies and notices by when they were made (`transcript`), replies first at the same
        // instant; the engine's order is `pos`. Two entries made in the same instant are a microsecond apart here.
        var last = Date.distantPast
        func date(_ text: String) -> Date {
            let made = max(EntryFold.date(text), last.addingTimeInterval(0.000001))
            last = made
            return made
        }
        for entry in ordered {
            let body = entry.body
            switch entry.kind {
            case "message":
                var turn = ConversationTurn(id: uuid(entry.id), createdAt: date(entry.createdAt),
                                            request: PasteMarkers.removed(from: body["text"] as? String ?? ""),
                                            sources: (body["attachments"] as? [String] ?? []).map { URL(fileURLWithPath: $0) })
                turn.requestID = entry.requestID
                turn.backendTaskID = entry.taskID
                if let hidden = body["hidden"] as? [String], !hidden.isEmpty { turn.hiddenKinds = hidden }
                switch entry.state {
                case "merged":
                    turn.status = "merged"
                    turn.mergedInto = body["merged_into"] as? String
                case "interrupted":
                    turn.status = "interrupted"
                    turn.responseNotice = body["notice"] as? String
                    turn.statusInferred = true
                default: break
                }
                turnOf[entry.id] = archive.turns.count
                if let request = entry.requestID { byRequest[request] = archive.turns.count }
                archive.turns.append(turn)
            case "plan":
                guard let index = entry.requestID.flatMap({ byRequest[$0] }) else { continue }
                showPlan(body, on: &archive.turns[index])
            case "answer", "error":
                let index: Int
                if let message = entry.answers, let found = turnOf[message] {
                    index = found
                } else if let found = turnOf[entry.id] {
                    index = found
                } else {
                    // FERB's own turn for a task this chat didn't send (recordRecovered), described by its goal
                    var row: [String: Any] = [:]
                    if let name = body["name"] as? String { row["name"] = name }
                    if let goal = body["goal"] as? String { row["goal"] = goal }
                    let turn = ConversationTurn(id: uuid(entry.id), createdAt: date(entry.createdAt), request: "",
                                                sources: [], backendTaskID: entry.taskID,
                                                taskGoal: ShortName.of(row, fallback: "goal"))
                    index = archive.turns.count
                    turnOf[entry.id] = index
                    archive.turns.append(turn)
                }
                if body["follows"] is String, let newest = archive.turns.last {
                    // a later answer (APP-914): said after the message's answer, in its own words, where it arrived
                    archive.replies.append(ConversationReply(id: uuid(entry.id), turnID: archive.turns[index].id,
                        createdAt: date(entry.createdAt), afterTurnID: newest.id, part: .answer, said: later(body)))
                    continue
                }
                if entry.placement == "below", let newest = archive.turns.last {
                    archive.replies.append(ConversationReply(id: uuid(entry.id), turnID: archive.turns[index].id,
                                                             createdAt: date(entry.createdAt), afterTurnID: newest.id,
                                                             part: (body["part"] as? String).flatMap(ReplyPart.init)))
                }
                // the message's own answer is its newest one that follows no other
                shown[index] = entry
            case "notice":
                let quotes = (body["quotes"] as? [[String: Any]] ?? []).compactMap { quote -> NoticeQuote? in
                    guard let text = quote["text"] as? String, !text.isEmpty else { return nil }
                    return NoticeQuote(text: text, origin: quote["origin"] as? String ?? "")
                }
                archive.notices.append(ConversationNotice(id: entry.ref ?? entry.id, entryID: uuid(entry.id),
                    createdAt: date(entry.createdAt), noticedAt: LiveProgress.time(body["at"]),
                    text: body["text"] as? String ?? "", quotes: quotes, tag: "noticed",
                    afterTurnID: archive.turns.last?.id, proposals: nil, cards: ResultCard.list(body["cards"])))
            case "post":
                // a reminder as receiveReminder shows it: its card from the banner's, its heading and kind
                let text = body["text"] as? String ?? ""
                let id = body["timer_id"] as? String ?? ""
                let card = ResultCard(body["card"])
                var shown: [String: Any] = ["type": "reminder", "title": card?.title ?? ShortName.clause(text),
                                            "kind": card?.kind ?? "reminder"]
                if let due = card?.due { shown["due"] = due }
                if let started = card?.started { shown["started"] = started }
                if let first = card?.repeatOf { shown["repeat_of"] = first }
                shown["timer_id"] = card?.timerID ?? id
                let made = date(entry.createdAt)
                archive.notices.append(ConversationNotice(id: entry.ref ?? "reminder:" + id, entryID: uuid(entry.id),
                    createdAt: made, noticedAt: made, text: text, quotes: [],
                    tag: "noticed", afterTurnID: archive.turns.last?.id, proposals: nil,
                    cards: ResultCard.list([shown])))
            case "legacy":
                guard let kind = body["kind"] as? String, let saved = body["body"],
                      let stored = HistoryEntry.read(["kind": kind, "body": saved]) else { continue }
                switch stored {
                case let .turn(turn):
                    turnOf[entry.id] = archive.turns.count
                    if let request = turn.requestID { byRequest[request] = archive.turns.count }
                    archive.turns.append(turn)
                case let .reply(reply): archive.replies.append(reply)
                case let .notice(notice): archive.notices.append(notice)
                }
            default: break      // a question (held apart by the app), or a kind this build doesn't know
            }
        }
        for (index, entry) in shown { answer(entry.body, on: &archive.turns[index]) }
        return archive
    }

    /// The archive of a JSON list of entries.
    public static func archive(json value: Any?) -> ConversationArchive { archive(ChatEntry.list(value)) }

    /// A plan's fields on its turn (showPlan's, as the engine kept them).
    static func showPlan(_ body: [String: Any], on turn: inout ConversationTurn) {
        if let text = body["text"] as? String { turn.plan = text }
        if body["name"] is String || body["goal"] is String {
            let name = ShortName.of(body, fallback: "goal")
            if !name.isEmpty { turn.taskName = name }
        }
        if let line = body["plan_line"] as? String {
            turn.planLine = line
            turn.planLines = body["lines"] as? [String] ?? []
        }
        if let direct = body["direct"] as? [String] { turn.direct = direct }
        if let receipts = body["receipts"] as? [String] { turn.receipts = receipts }
        if let cards = ResultCard.list(body["receipt_cards"]) { turn.receiptCards = cards }
        if let card = body["card"] as? [String: Any], let taskID = card["task_id"] as? String {
            turn.card = TurnCard(card, taskID: taskID)
        }
        if let saved = body["checklist"] as? [String: Any], let taskID = saved["task_id"] as? String,
           var checklist = StepChecklist(["steps": saved["steps"] ?? []], taskID: taskID) {
            if saved["finished"] as? Bool == true {
                let ended = (saved["ended"] as? [String: String] ?? [:]).mapValues { ["status": $0] }
                checklist.end(StepChecklist(["steps": saved["steps"] ?? [], "states": ended], taskID: taskID))
            }
            turn.checklist = checklist
        }
    }

    /// An answer's fields on its turn (finish's, applyOutcome's, a reply's cards and offer).
    static func answer(_ body: [String: Any], on turn: inout ConversationTurn) {
        turn.answer = body["text"] as? String ?? ""
        turn.displayText = body["display_text"] as? String
        turn.status = body["status"] as? String ?? "done"
        turn.artifacts = (body["artifacts"] as? [String] ?? []).map { URL(fileURLWithPath: $0) }
        turn.verified = body["verified"] as? Bool ?? false
        turn.responseNotice = nil
        turn.statusInferred = nil
        turn.note = body["note"] as? String
        turn.codeLines = body["lines"] as? [String]
        turn.runtimeSwitch = RuntimeSwitch(body["runtime_switch"])
        turn.cards = ResultCard.list(body["cards"])
        turn.question = body["question"] as? String
    }

    /// A later answer's own fields (APP-914), as answerLater keeps them.
    static func later(_ body: [String: Any]) -> LaterAnswer {
        LaterAnswer(answer: body["text"] as? String ?? "", displayText: body["display_text"] as? String,
                    status: body["status"] as? String ?? "done", verified: body["verified"] as? Bool ?? false,
                    artifacts: (body["artifacts"] as? [String] ?? []).map { URL(fileURLWithPath: $0) },
                    note: body["note"] as? String, codeLines: body["lines"] as? [String],
                    cards: ResultCard.list(body["cards"]), runtimeSwitch: RuntimeSwitch(body["runtime_switch"]))
    }

    /// The engine's times (ISO 8601 with an offset).
    static func date(_ text: String) -> Date { LiveProgress.time(text) ?? Date(timeIntervalSince1970: 0) }

    /// A stable UUID for an entry id, so a reply names its turn's.
    static func uuid(_ id: String) -> UUID {
        func fnv(_ seed: UInt64) -> UInt64 {
            var hash = seed
            for byte in id.utf8 { hash = (hash ^ UInt64(byte)) &* 0x100000001b3 }
            return hash
        }
        let a = fnv(0xcbf29ce484222325), b = fnv(0x84222325cbf29ce4)
        var bytes = [UInt8](repeating: 0, count: 16)
        for i in 0..<8 {
            bytes[i] = UInt8(truncatingIfNeeded: a >> (8 * UInt64(i)))
            bytes[8 + i] = UInt8(truncatingIfNeeded: b >> (8 * UInt64(i)))
        }
        return UUID(uuid: (bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8],
                           bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]))
    }
}
