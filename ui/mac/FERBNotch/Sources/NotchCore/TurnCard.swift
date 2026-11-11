import Foundation

/// Answers in steps (ORCH-319, 20): a request with several asks gets one card, a row per ask (part),
/// each filled once in the owner's order. The daemon sends the card with its plan line, each row's change as a `part`
/// event (on the turn's stream and on the bus as `task.part`), and the footer with the outcome. Pure state: the notch
/// draws it (ConversationTranscript's card).

/// A question FERB asks inside a row (asks, paused): the approval it answers, its words and its choices.
public struct RowQuestion: Codable, Equatable {
    public let approvalID: String
    public let text: String
    public let options: [String]

    public init?(_ value: Any?) {
        guard let value = value as? [String: Any], let id = value["approval_id"] as? String, !id.isEmpty else { return nil }
        approvalID = id
        text = value["text"] as? String ?? ""
        options = value["options"] as? [String] ?? []
    }

    private enum CodingKeys: String, CodingKey { case approvalID, text, options }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        approvalID = try c.decode(String.self, forKey: .approvalID)
        text = try c.value(.text, or: "")
        options = try c.value(.options, or: [])
    }
}

/// One part of the owner's request in the card. Each `part` event is the row's whole state now (the daemon sends the
/// posted fields with a filled row and the live ones with a working, asking or paused row), so applying one replaces
/// what the row said.
public struct CardRow: Codable, Equatable, Identifiable {
    public let taskID: String
    public let part: String
    public var label: String
    /// working, done, failed, stopped (the owner's No on its action's card, ORCH-613), asks, paused or updated.
    public var state: String
    /// The answer; a failed row's "Couldn't do: …"; a paused row's reason.
    public var text: String?
    /// "Couldn't confirm: …", without its prefix.
    public var note: String?
    /// Why an updated row changed, without the "Updated:" prefix.
    public var why: String?
    /// The owner's answer to the row's question, while it works on with it.
    public var chosen: String?
    public var verified: Bool?
    /// Answered in the first reply (its text is in the plan bubble).
    public var direct: Bool
    /// The step that answers it, and the task id that step's worker runs under: a confirmation for that task id, or one
    /// under it, belongs to this row; the live preview of this row's work is asked for with it.
    public var step: String?
    public var stepTaskID: String?
    /// The daemon's order of this row's changes (ISO UTC time to the microsecond): an event applies only when it sorts
    /// after the row's, so the same change arriving on the stream and on the bus is applied once.
    public var seq: String?
    public var question: RowQuestion?
    /// The turn whose card first showed this part.
    public var cardRequestID: String?

    public var id: String { part }
    public var isDone: Bool { state == "done" || state == "updated" }
    /// Filled, failed, stopped by the owner (ORCH-613) or updated: nothing more is coming for it in this run.
    public var isSettled: Bool { isDone || state == "failed" || state == "stopped" }

    public init?(_ value: [String: Any], taskID fallback: String) {
        guard let part = value["part"] as? String, !part.isEmpty else { return nil }
        taskID = (value["task_id"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? fallback
        self.part = part
        label = value["label"] as? String ?? ""
        state = value["state"] as? String ?? "working"
        direct = value["direct"] as? Bool ?? false
        step = value["step"] as? String
        stepTaskID = value["step_task_id"] as? String
        cardRequestID = value["card_request_id"] as? String
        seq = value["seq"] as? String
        take(value)
    }

    private enum CodingKeys: String, CodingKey {
        case taskID, part, label, state, text, note, why, chosen, verified, direct, step, stepTaskID, seq, question
        case cardRequestID
    }

    /// Read tolerantly (APP-801): only its task and part are required; a question that can't be read is left out.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        taskID = try c.decode(String.self, forKey: .taskID)
        part = try c.decode(String.self, forKey: .part)
        label = try c.value(.label, or: "")
        state = try c.value(.state, or: "working")
        text = try c.decodeIfPresent(String.self, forKey: .text)
        note = try c.decodeIfPresent(String.self, forKey: .note)
        why = try c.decodeIfPresent(String.self, forKey: .why)
        chosen = try c.decodeIfPresent(String.self, forKey: .chosen)
        verified = try c.decodeIfPresent(Bool.self, forKey: .verified)
        direct = try c.value(.direct, or: false)
        step = try c.decodeIfPresent(String.self, forKey: .step)
        stepTaskID = try c.decodeIfPresent(String.self, forKey: .stepTaskID)
        seq = try c.decodeIfPresent(String.self, forKey: .seq)
        question = try c.lossy(RowQuestion.self, .question)
        cardRequestID = try c.decodeIfPresent(String.self, forKey: .cardRequestID)
    }

    /// The row's changeable fields from an event or a snapshot row: absent ones are cleared (each is the whole state).
    private mutating func take(_ value: [String: Any]) {
        func words(_ key: String) -> String? { (value[key] as? String).flatMap { $0.isEmpty ? nil : $0 } }
        text = words("text")
        note = words("note")
        why = words("why")
        chosen = words("chosen")
        verified = value["verified"] as? Bool
        question = (state == "asks" || state == "paused") ? RowQuestion(value["question"]) : nil
    }

    /// Applies a change of this row when it is newer than what the row shows. Returns whether it applied.
    @discardableResult
    public mutating func apply(_ value: [String: Any]) -> Bool {
        guard Self.sortsAfter(value["seq"] as? String, seq) else { return false }
        if let label = value["label"] as? String, !label.isEmpty { self.label = label }
        state = value["state"] as? String ?? state
        if let step = value["step"] as? String { self.step = step }
        if let stepTask = value["step_task_id"] as? String, !stepTask.isEmpty { stepTaskID = stepTask }
        if let card = value["card_request_id"] as? String, !card.isEmpty { cardRequestID = card }
        seq = value["seq"] as? String ?? seq
        take(value)
        return true
    }

    /// `new` is a later change than `current`. A change without a seq (an older daemon) applies only to a row that has
    /// none either.
    public static func sortsAfter(_ new: String?, _ current: String?) -> Bool {
        guard let new, !new.isEmpty else { return current?.isEmpty ?? true }
        guard let current, !current.isEmpty else { return true }
        return new > current
    }

    /// Whether this open question belongs in this row: FERB's own question for this part (it names the part, or is
    /// the row's question), or a confirmation the gateway asks for the row's step task or a task under it.
    public func asks(_ approval: DaemonApproval) -> Bool {
        if question?.approvalID == approval.id { return true }
        if let part = approval.part, part == self.part, approval.taskID == taskID { return true }
        guard let step = stepTaskID, !step.isEmpty, let source = approval.sourceTaskID else { return false }
        return source == step || source.hasPrefix(step + ".")
    }
}

/// The card of one turn of a multi-part task: its rows in the owner's order and, once the task ended, the footer
/// code wrote ("2 of 3 done", "Sites contacted: …") and a stop's headline.
public struct TurnCard: Codable, Equatable {
    public let taskID: String
    /// The turn (request) whose card this is.
    public let requestID: String
    public private(set) var rows: [CardRow]
    /// Over all of the task's asks (a continued run's card shows only the parts it works on).
    public var done: Int
    public var total: Int
    /// The footer's done line; set when the task ended.
    public var line: String?
    public var lines: [String]
    public var headline: String?
    /// Each plan step's state, by step id (ORCH-531): live state only, never saved (history shows no live states).
    public var steps: [String: StepState] = [:]

    public var finished: Bool { line != nil }

    private enum CodingKeys: String, CodingKey { case taskID, requestID, rows, done, total, line, lines, headline }

    public init?(_ payload: Any?, taskID: String) {
        guard let payload = payload as? [String: Any], let rows = payload["rows"] as? [[String: Any]] else { return nil }
        self.taskID = taskID
        requestID = payload["request_id"] as? String ?? ""
        self.rows = rows.compactMap { CardRow($0, taskID: taskID) }
        done = (payload["done"] as? NSNumber)?.intValue ?? 0
        total = (payload["total"] as? NSNumber)?.intValue ?? self.rows.count
        line = (payload["line"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        lines = payload["lines"] as? [String] ?? []
        headline = (payload["headline"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        for (key, value) in payload["steps"] as? [String: Any] ?? [:] {
            if let state = StepState(value) { steps[key] = state }
        }
    }

    /// Read tolerantly (APP-801): only its task is required; a row that can't be read is left out, not the card.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        taskID = try c.decode(String.self, forKey: .taskID)
        requestID = try c.value(.requestID, or: "")
        rows = try c.lossyList(CardRow.self, .rows) ?? []
        done = try c.value(.done, or: 0)
        total = try c.value(.total, or: rows.count)
        line = try c.decodeIfPresent(String.self, forKey: .line)
        lines = try c.value(.lines, or: [])
        headline = try c.decodeIfPresent(String.self, forKey: .headline)
    }

    /// Applies a `step` event (ORCH-531) when it is newer than the step's state shown. Returns whether it applied.
    @discardableResult
    public mutating func apply(step event: [String: Any]) -> Bool {
        guard let key = event["step"] as? String, !key.isEmpty, let state = StepState(event),
              StepState.replaces(state, steps[key]), steps[key] != state else { return false }
        steps[key] = state
        return true
    }

    public func row(_ part: String) -> CardRow? { rows.first { $0.part == part } }

    /// The rows the card draws: every row, a part answered at once included, so they always agree with "N of M"
    /// (ORCH-543). TurnCardView and the playground's play read this one list.
    public var shownRows: [CardRow] { rows }

    /// Applies a part event to its row. A part this card doesn't list is added when the event names this card's turn.
    /// The count is the daemon's (`done`, `total` on the event, step 8e); an older daemon's event without them is
    /// counted here as before. Returns whether anything changed.
    @discardableResult
    public mutating func apply(part event: [String: Any]) -> Bool {
        guard let part = event["part"] as? String else { return false }
        if let index = rows.firstIndex(where: { $0.part == part }) {
            let wasDone = rows[index].isDone
            guard rows[index].apply(event) else { return false }
            if !takeCount(event) {
                done = max(0, min(total, done + (rows[index].isDone ? 1 : 0) - (wasDone ? 1 : 0)))
            }
            return true
        }
        guard event["request_id"] as? String == requestID, let row = CardRow(event, taskID: taskID) else { return false }
        rows.append(row)
        if !takeCount(event) {
            total = max(total, rows.count)
            if row.isDone { done = min(total, done + 1) }
        }
        return true
    }

    /// The daemon's count after this change, when the event carries it.
    private mutating func takeCount(_ event: [String: Any]) -> Bool {
        guard let done = (event["done"] as? NSNumber)?.intValue, let total = (event["total"] as? NSNumber)?.intValue
        else { return false }
        self.total = max(total, rows.count)
        self.done = max(0, min(self.total, done))
        return true
    }

    /// A newer picture of the same card (the plan event again, `chat.status`, the outcome): each row is taken when it
    /// is newer than the one shown (a live event may have arrived after the snapshot was made); the counts and the
    /// footer come from it.
    public mutating func merge(_ snapshot: TurnCard) {
        var merged: [CardRow] = []
        for incoming in snapshot.rows {
            if let shown = row(incoming.part), !CardRow.sortsAfter(incoming.seq, shown.seq) {
                merged.append(shown)
            } else {
                merged.append(incoming)
            }
        }
        merged += rows.filter { shown in !snapshot.rows.contains { $0.part == shown.part } }
        rows = merged
        for (key, state) in snapshot.steps where StepState.replaces(state, steps[key]) { steps[key] = state }
        done = snapshot.done
        total = max(snapshot.total, rows.count)
        if snapshot.line != nil {
            line = snapshot.line
            lines = snapshot.lines
            headline = snapshot.headline
        }
    }
}

/// The offer of another signed-in AI service when an account fact (a usage limit, a sign-in problem) ended the work
/// (ORCH-315): the runtime that failed, the role it served and the other runtimes FERB could use.
public struct RuntimeSwitch: Codable, Equatable {
    public let runtime: String
    public let problem: String
    public let role: String
    public let choices: [String]

    public init?(_ value: Any?) {
        guard let value = value as? [String: Any], let choices = value["choices"] as? [String], !choices.isEmpty else { return nil }
        runtime = value["runtime"] as? String ?? ""
        problem = value["problem"] as? String ?? ""
        role = value["role"] as? String ?? "worker"
        self.choices = choices
    }

    private enum CodingKeys: String, CodingKey { case runtime, problem, role, choices }

    /// Read tolerantly (APP-801): an offer needs its choices; the rest reads as a new offer's.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        runtime = try c.value(.runtime, or: "")
        problem = try c.value(.problem, or: "")
        role = try c.value(.role, or: "worker")
        choices = try c.decode([String].self, forKey: .choices)
    }

    /// The runtime's name as FERB says it (ferb/runtimes/catalog.py LABELS).
    public static func label(_ name: String) -> String {
        ["codex": "Codex", "claude": "Claude", "anthropic": "the Anthropic API", "openai": "the OpenAI API",
         "google": "the Google API"][name] ?? name
    }
}

/// A one-ask answer split into what it says and code's own lines after it (ORCH-319's structured fields), so the
/// notch draws the "Couldn't confirm" line and code's lines on their own. The daemon also writes them
/// into `answer` (history, voice, older clients): what is drawn on its own is cut from the end of the text, and a piece
/// that isn't found there stays in the text and isn't drawn again.
public struct AnswerParts: Equatable {
    public let body: String
    public let note: String?
    public let lines: [String]

    public init(answer: String, note: String?, lines: [String]) {
        var body = answer
        var shownLines: [String] = []
        if !lines.isEmpty {
            let joined = lines.joined(separator: "\n")
            if let range = body.range(of: joined, options: .backwards),
               range.lowerBound == body.startIndex || body[..<range.lowerBound].hasSuffix("\n") {
                // A line the daemon adds after them ("(ran late)") stays with them.
                let after = body[range.upperBound...].split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }
                shownLines = lines + after.filter { !$0.isEmpty }
                body = String(body[..<range.lowerBound])
            }
        }
        body = body.trimmingCharacters(in: .whitespacesAndNewlines)
        let note = note.flatMap { $0.isEmpty ? nil : $0 }
        var shownNote: String?
        if let note {
            let line = "Couldn't confirm: \(note)."
            if body.hasSuffix(line) {
                body = String(body.dropLast(line.count)).trimmingCharacters(in: .whitespacesAndNewlines)
                shownNote = note
            }
        }
        self.body = body
        self.note = shownNote
        self.lines = shownLines
    }
}
