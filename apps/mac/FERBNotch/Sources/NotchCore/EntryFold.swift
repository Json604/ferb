import Foundation

// Stage 2 of the Rust engine (ENG-8): the engine keeps the chat as entries (`ferb_core::chat_entry`), made where it
// sends today's `chat.send` frames and bus events, so every client reads the same conversation. This file reads those
// entries and folds them into the archive FERB.app draws today (`ConversationArchive`). `EntryFoldTests` proves the
// fold gives the transcript the app's own reducer builds from the same frames. FERB.app keeps writing its own turns
// (ENG-10); it applies, entry by entry and with the same per-entry mapping, only the turns another client sent
// (`applyMessageEntry`, `applyPlanEntry`, `applyAnswerEntry`, called from `EntryWatch.swift`).

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
    /// The client that sent the message of this entry's turn (every entry of that turn carries it); nil when it sent
    /// none, or for an entry of no message's turn.
    public let clientID: String?
    public let submissionID: String?
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
        clientID = words("client_id")
        submissionID = words("submission_id")
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
            switch entry.kind {
            case "message":
                let turn = messageTurn(entry, createdAt: date(entry.createdAt))
                turnOf[entry.id] = archive.turns.count
                if let request = entry.requestID { byRequest[request] = archive.turns.count }
                archive.turns.append(turn)
            case "plan":
                guard let index = entry.requestID.flatMap({ byRequest[$0] }) else { continue }
                showPlan(entry.body, on: &archive.turns[index])
            case "answer", "error":
                let index: Int
                if let message = entry.answers, let found = turnOf[message] {
                    index = found
                } else if let found = turnOf[entry.id] {
                    index = found
                } else {
                    index = archive.turns.count
                    turnOf[entry.id] = index
                    archive.turns.append(recoveredTurn(entry, createdAt: date(entry.createdAt)))
                }
                if follows(entry), let newest = archive.turns.last {
                    archive.replies.append(laterReply(entry, turnID: archive.turns[index].id, afterTurnID: newest.id,
                                                      createdAt: date(entry.createdAt)))
                    continue
                }
                if entry.placement == "below", let newest = archive.turns.last {
                    archive.replies.append(belowReply(entry, turnID: archive.turns[index].id, afterTurnID: newest.id,
                                                      createdAt: date(entry.createdAt)))
                }
                // the message's own answer is its newest one that follows no other
                shown[index] = entry
            // APP-1106: an alert (marked by the engine, or named so by its id) is never drawn in the chat
            case "notice" where isAlert(entry), "legacy" where isAlert(entry):
                continue
            case "notice":
                archive.notices.append(notice(entry, createdAt: date(entry.createdAt), afterTurnID: archive.turns.last?.id))
            case "post":
                archive.notices.append(post(entry, createdAt: date(entry.createdAt), afterTurnID: archive.turns.last?.id))
            case "legacy":
                guard let kind = entry.body["kind"] as? String, let saved = entry.body["body"],
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

    // MARK: one entry at a time (the fold above and the app's watch, `ConversationArchive.apply…Entry`, share these)

    /// A message's turn: the owner's words as the engine recorded them, its files, request and task, a merged or
    /// interrupted message's state.
    public static func messageTurn(_ entry: ChatEntry, createdAt: Date) -> ConversationTurn {
        let body = entry.body
        var turn = ConversationTurn(id: uuid(entry.id), createdAt: createdAt,
                                    request: PasteMarkers.removed(from: body["text"] as? String ?? ""),
                                    sources: (body["attachments"] as? [String] ?? []).map { URL(fileURLWithPath: $0) })
        turn.requestID = entry.requestID
        turn.backendTaskID = entry.taskID
        if let hidden = body["hidden"] as? [String], !hidden.isEmpty { turn.hiddenKinds = hidden }
        turn.sealed = sealed(body)
        messageState(entry, on: &turn)
        return turn
    }

    /// The IDs kept sealed from a message (GATE-604: the ack's `sealed`, kept on its entry), as the ack shows them; nil
    /// when none.
    static func sealed(_ body: [String: Any]) -> [SealedIdentifier]? {
        let kept = (body["sealed"] as? [[String: Any]] ?? []).compactMap(SealedIdentifier.init)
        return kept.isEmpty ? nil : kept
    }

    /// A merged or interrupted message's state on its turn.
    static func messageState(_ entry: ChatEntry, on turn: inout ConversationTurn) {
        switch entry.state {
        case "merged":
            turn.status = "merged"
            turn.mergedInto = entry.body["merged_into"] as? String
        case "interrupted":
            turn.status = "interrupted"
            turn.responseNotice = entry.body["notice"] as? String
            turn.statusInferred = true
        default: break
        }
    }

    /// FERB's own turn for a task this chat didn't send (recordRecovered), described by its goal.
    public static func recoveredTurn(_ entry: ChatEntry, createdAt: Date) -> ConversationTurn {
        var row: [String: Any] = [:]
        if let name = entry.body["name"] as? String { row["name"] = name }
        if let goal = entry.body["goal"] as? String { row["goal"] = goal }
        return ConversationTurn(id: uuid(entry.id), createdAt: createdAt, request: "", sources: [],
                                backendTaskID: entry.taskID, taskGoal: ShortName.of(row, fallback: "goal"))
    }

    /// Whether an answer is a later answer (APP-914): said after the message's answer, in its own words.
    public static func follows(_ entry: ChatEntry) -> Bool { entry.body["follows"] is String }

    /// A later answer (APP-914) as a reply of its own, where it arrived; its id is the entry's, so a revision finds it.
    public static func laterReply(_ entry: ChatEntry, turnID: UUID, afterTurnID: UUID, createdAt: Date) -> ConversationReply {
        ConversationReply(id: uuid(entry.id), turnID: turnID, createdAt: createdAt, afterTurnID: afterTurnID,
                          part: .answer, said: later(entry.body))
    }

    /// An answer placed below (APP-580): a reply quoting its turn, which shows that turn's answer.
    public static func belowReply(_ entry: ChatEntry, turnID: UUID, afterTurnID: UUID, createdAt: Date) -> ConversationReply {
        ConversationReply(id: uuid(entry.id), turnID: turnID, createdAt: createdAt, afterTurnID: afterTurnID,
                          part: (entry.body["part"] as? String).flatMap(ReplyPart.init))
    }

    /// One of FERB's alerts (APP-1106), as ferb-mobile's fold reads it: marked `alert` by the engine, or a notice whose
    /// ref is an `alert:` id.
    static func isAlert(_ entry: ChatEntry) -> Bool {
        entry.body["alert"] as? Bool == true || (entry.kind == "notice" && entry.ref?.hasPrefix("alert:") == true)
    }

    /// A notice as receiveNotices shows it.
    public static func notice(_ entry: ChatEntry, createdAt: Date, afterTurnID: UUID?) -> ConversationNotice {
        let body = entry.body
        let quotes = (body["quotes"] as? [[String: Any]] ?? []).compactMap { quote -> NoticeQuote? in
            guard let text = quote["text"] as? String, !text.isEmpty else { return nil }
            return NoticeQuote(text: text, origin: quote["origin"] as? String ?? "")
        }
        return ConversationNotice(id: entry.ref ?? entry.id, entryID: uuid(entry.id), createdAt: createdAt,
                                  noticedAt: LiveProgress.time(body["at"]), text: body["text"] as? String ?? "",
                                  quotes: quotes, tag: "noticed", afterTurnID: afterTurnID, proposals: nil,
                                  cards: ResultCard.list(body["cards"]))
    }

    /// A reminder as receiveReminder shows it: its card from the banner's, its heading and kind.
    public static func post(_ entry: ChatEntry, createdAt: Date, afterTurnID: UUID?) -> ConversationNotice {
        let body = entry.body
        let text = body["text"] as? String ?? ""
        let id = body["timer_id"] as? String ?? ""
        let card = ResultCard(body["card"])
        var shown: [String: Any] = ["type": "reminder", "title": card?.title ?? ShortName.clause(text),
                                    "kind": card?.kind ?? "reminder"]
        if let due = card?.due { shown["due"] = due }
        if let started = card?.started { shown["started"] = started }
        if let first = card?.repeatOf { shown["repeat_of"] = first }
        shown["timer_id"] = card?.timerID ?? id
        return ConversationNotice(id: entry.ref ?? "reminder:" + id, entryID: uuid(entry.id), createdAt: createdAt,
                                  noticedAt: createdAt, text: text, quotes: [], tag: "noticed",
                                  afterTurnID: afterTurnID, proposals: nil, cards: ResultCard.list([shown]))
    }

    /// A plan's fields on its turn (showPlan's, as the engine kept them).
    public static func showPlan(_ body: [String: Any], on turn: inout ConversationTurn) {
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
    public static func answer(_ body: [String: Any], on turn: inout ConversationTurn) {
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
    public static func later(_ body: [String: Any]) -> LaterAnswer {
        LaterAnswer(answer: body["text"] as? String ?? "", displayText: body["display_text"] as? String,
                    status: body["status"] as? String ?? "done", verified: body["verified"] as? Bool ?? false,
                    artifacts: (body["artifacts"] as? [String] ?? []).map { URL(fileURLWithPath: $0) },
                    note: body["note"] as? String, codeLines: body["lines"] as? [String],
                    cards: ResultCard.list(body["cards"]), runtimeSwitch: RuntimeSwitch(body["runtime_switch"]))
    }

    /// The engine's times (ISO 8601 with an offset).
    public static func date(_ text: String) -> Date { LiveProgress.time(text) ?? Date(timeIntervalSince1970: 0) }

    /// A stable UUID for an entry id, so a reply names its turn's.
    public static func uuid(_ id: String) -> UUID {
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

// MARK: one entry applied to the chat the app holds (ENG-10)

extension ConversationArchive {
    /// The newest moment any entry here was made.
    private var newestDate: Date {
        [turns.last?.createdAt, replies.last?.createdAt, notices.last?.createdAt].compactMap { $0 }.max() ?? .distantPast
    }

    /// When an entry made now is placed: its own time, after everything already here (as the fold dates entries in
    /// their order: the same instant twice is a microsecond apart).
    private func stamp(_ entry: ChatEntry) -> Date {
        max(EntryFold.date(entry.createdAt), newestDate.addingTimeInterval(0.000001))
    }

    /// A message entry (ENG-10): the turn holding its request (or made from this entry before) takes its newest words,
    /// files' secrets notice, task and state; else its turn is made as the fold makes it and put at `index` of the
    /// turns, dated between its neighbours so a relaunch reads it back in the same place. `elsewhere`: another client
    /// sent it (`sentElsewhere`). A merged or interrupted state changes a turn that has no answer yet only. Returns the
    /// turn when anything changed.
    @discardableResult
    public mutating func applyMessageEntry(_ entry: ChatEntry, at index: Int, elsewhere: Bool) -> UUID? {
        guard entry.kind == "message", !entry.deleted else { return nil }
        let id = EntryFold.uuid(entry.id)
        if let found = turns.firstIndex(where: { (entry.requestID != nil && $0.requestID == entry.requestID) || $0.id == id }) {
            var turn = turns[found]
            let text = PasteMarkers.removed(from: entry.body["text"] as? String ?? "")
            if !text.isEmpty {
                turn.request = text
                turn.awaitingRedaction = false
                turn.taskGoal = nil          // a result filed before its message came is that message's turn
            }
            if turn.requestID == nil { turn.requestID = entry.requestID }
            if let hidden = entry.body["hidden"] as? [String], !hidden.isEmpty { turn.hiddenKinds = hidden }
            if let sealed = EntryFold.sealed(entry.body) {
                // the owner's "Let the AI see" on this Mac stays (GATE-602)
                turn.sealed = sealed.map { kept in
                    var kept = kept
                    kept.shown = turn.sealed?.first { $0.name == kept.name }?.shown ?? false
                    return kept
                }
            }
            if let task = entry.taskID { turn.backendTaskID = task }
            if turn.answer.isEmpty, ["working", "interrupted", "merged"].contains(turn.status) {
                EntryFold.messageState(entry, on: &turn)
            }
            if elsewhere { turn.sentElsewhere = true }
            guard turn != turns[found] else { return nil }
            turns[found] = turn
            return turn.id
        }
        let at = min(max(index, 0), turns.count)
        var made = EntryFold.date(entry.createdAt)
        if at == turns.count {
            made = stamp(entry)
        } else {
            if at > 0 { made = max(made, turns[at - 1].createdAt.addingTimeInterval(0.000001)) }
            made = min(made, turns[at].createdAt.addingTimeInterval(-0.000001))
        }
        var turn = EntryFold.messageTurn(entry, createdAt: made)
        if elsewhere { turn.sentElsewhere = true }
        turns.insert(turn, at: at)
        return turn.id
    }

    /// A plan entry on the turn of its request (the fold's `showPlan`), its task bound. A newer picture of the same
    /// card or checklist merges into the one shown, as the app's own plan does, so live step states stay. Returns the
    /// turn when anything changed.
    @discardableResult
    public mutating func applyPlanEntry(_ entry: ChatEntry) -> UUID? {
        guard entry.kind == "plan", !entry.deleted, let request = entry.requestID,
              let index = turns.lastIndex(where: { $0.requestID == request }) else { return nil }
        var turn = turns[index]
        let card = turn.card, checklist = turn.checklist
        EntryFold.showPlan(entry.body, on: &turn)
        if var shown = card, let incoming = turn.card, incoming != shown, shown.requestID == incoming.requestID,
           shown.taskID == incoming.taskID {
            shown.merge(incoming)
            turn.card = shown
        }
        if let shown = checklist, let incoming = turn.checklist, shown.taskID == incoming.taskID, !incoming.finished {
            if shown.finished {
                turn.checklist = shown
            } else {
                var kept = shown
                kept.merge(incoming)
                turn.checklist = kept
            }
        }
        if let task = entry.taskID { turn.backendTaskID = task }
        turn.awaitingReply = false
        turn.streamingPlan = nil
        turn.liveLine = nil
        guard turn != turns[index] else { return nil }
        turns[index] = turn
        return turn.id
    }

    /// An answer or error entry on the turn of its request, as the fold places it: a later answer (APP-914) is a reply
    /// of its own, found again by its entry's id when revised; an answer placed below adds the reply quoting its turn
    /// (APP-580); the turn shows its newest answer that follows no other (`shown`: that answer's pos, by request). The
    /// live line goes. Returns the turn when anything changed.
    @discardableResult
    public mutating func applyAnswerEntry(_ entry: ChatEntry, shown: inout [String: Int]) -> UUID? {
        guard ["answer", "error"].contains(entry.kind), !entry.deleted, let request = entry.requestID,
              let index = turns.lastIndex(where: { $0.requestID == request }), let newest = turns.last else { return nil }
        let before = (turns[index], replies)
        let turnID = turns[index].id
        let replyID = EntryFold.uuid(entry.id)
        if EntryFold.follows(entry) {
            if let found = replies.firstIndex(where: { $0.id == replyID }) {
                replies[found].said = EntryFold.later(entry.body)
            } else {
                replies.append(EntryFold.laterReply(entry, turnID: turnID, afterTurnID: newest.id, createdAt: stamp(entry)))
            }
        } else {
            if entry.placement == "below", !replies.contains(where: { $0.id == replyID }) {
                replies.append(EntryFold.belowReply(entry, turnID: turnID, afterTurnID: newest.id, createdAt: stamp(entry)))
            }
            if entry.pos >= shown[request] ?? Int.min {
                shown[request] = entry.pos
                EntryFold.answer(entry.body, on: &turns[index])
            }
        }
        turns[index].liveLine = nil
        turns[index].awaitingReply = false
        turns[index].streamingPlan = nil
        return turns[index] != before.0 || replies != before.1 ? turnID : nil
    }
}
