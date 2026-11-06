import Foundation

public struct ConversationTurn: Identifiable, Codable, Equatable {
    public let id: UUID
    public let createdAt: Date
    /// The message the owner sent. It is saved with the conversation (History shows both sides); the unsent
    /// composer draft never is (L20).
    public internal(set) var request: String
    /// True from sending until the daemon's `ack` returns the message as it redacted it (L7). While true, `request`
    /// is the owner's raw typed text: memory only, never saved (rule 14). Not coded.
    public internal(set) var awaitingRedaction = false
    public let sources: [URL]
    public var backendTaskID: String?
    public var answer = ""
    public var displayText: String?
    public var visibleAnswer: String { displayText.flatMap { $0.isEmpty ? nil : $0 } ?? answer }
    public var status = "working"
    public var artifacts: [URL] = []
    public var responseNotice: String?
    public var isProvisional: Bool { status == "working" && backendTaskID == nil && !answer.isEmpty }
    /// The daemon's `request_id` for this message. Every contract event names it, so it ties events to this turn.
    public var requestID: String?
    /// The daemon's instant acknowledgement (`ack`, L7).
    public var ack: String?
    /// The plan line: the planner's words when it returned a task (`plan`, L7).
    public var plan: String?
    /// Each finished part of the request as text, from chats saved before cards (2026-10-27); shown, never added to.
    public var parts: [String]?
    /// The outcome's `verified` flag, shown as a small check mark.
    public var verified: Bool?
    /// The one live step line under the plan line (`progress`, L8). Live state only: never saved.
    public var liveLine: LiveProgress? = nil
    /// The question FERB asked (a `needs_user` answer), kept where it was shown once the final answer arrives.
    public var question: String?
    /// The notch inferred this turn's status itself (`interrupted`: its stream ended or its task stopped running with
    /// no confirmed answer from FERB); nil once FERB's own answer arrives. Saved, so a restart keeps it an inference.
    public var statusInferred: Bool?
    /// The daemon's goal for a task this chat didn't send (reconnected, recovered or started elsewhere); empty when
    /// unknown. Such a turn has no owner message: the goal is FERB's description, never words the owner typed.
    public var taskGoal: String?
    /// The task's short name (T-1055: the plan event's `name`), its heading in the task list; nil for a turn with no
    /// task or saved before names.
    public var taskName: String?
    /// The plan line on its own and code's lines under it ("Working in …"), from the plan event's fields; nil for a
    /// turn saved before they existed (its `plan` text is shown).
    public var planLine: String?
    public var planLines: [String]?
    /// A multi-part request's first reply (ORCH-319): the asks answered at once, then code's receipts (Remembered,
    /// 🗓 Scheduled), above the plan line.
    public var direct: [String]?
    public var receipts: [String]?
    /// A multi-part request's card (ORCH-319): a row per ask, filled in place, and the footer once the task ended.
    public var card: TurnCard?
    /// A one-ask answer's structured lines (ORCH-319): "Couldn't confirm: …" and code's lines. A turn saved with the
    /// "Correction: …" field older daemons sent decodes without it (ORCH-540: no answer carries one).
    public var note: String?
    public var codeLines: [String]?
    /// The offer of another AI service when an account problem ended the work (ORCH-315).
    public var runtimeSwitch: RuntimeSwitch?
    /// Typed cards (step 8e) of the reply or the outcome: drawn in place of the lines they stand for. Nil for a turn
    /// saved before cards, which draws its text as before.
    public var cards: [ResultCard]?
    /// The first reply's cards (a multi-part request's receipts), drawn in place of those receipts.
    public var receiptCards: [ResultCard]?
    /// The request this message was read together with (`merged`, ORCH-318): it has no reply of its own.
    public var mergedInto: String?
    /// The kinds of secret the daemon hid in this message (the ack's `hidden`: "saved secret" or a format's name, never
    /// a value); the notice under the message says so (`hiddenNotice`).
    public var hiddenKinds: [String]?
    /// The plan line as it streams (`plan_delta`), until the plan event replaces it. Live state only: never saved.
    public var streamingPlan: String? = nil
    /// FERB acknowledged the message and hasn't said anything else yet: the typing dots show. Live state only.
    public var awaitingReply = false
    /// The intro's look-don't-act preview of a suggested task (kb area app): its rows say what FERB
    /// would do; nothing was done. Never saved (`forStorage`).
    public var preview = false

    /// The typing-dots bubble (APP-300): from the acknowledgement until FERB's first words (a streamed plan line or
    /// reply, the plan, the reply).
    public var showsTyping: Bool {
        awaitingReply && status == "working" && answer.isEmpty && plan == nil && (streamingPlan ?? "").isEmpty
            && mergedInto == nil && card == nil
    }
    /// The first reply's bubble above a card (APP-755, owner 2026-11-06: "keep the row in the card. and dont show in
    /// the above reply"): a part answered at once shows only as its own card row, so with a card the bubble says no
    /// direct answer, and the planner's own copy of one is taken out of the plan line (a line or paragraph that is exactly
    /// its text, ORCH-549; a turn saved before this reads the same). Without a card nothing changes.
    public var shownDirect: [String] { card == nil ? (direct ?? []) : [] }
    public var shownPlanLine: String? { planLine.map(withoutDirect) }
    public var shownPlan: String? { plan.map(withoutDirect) }
    /// Whether the first reply's bubble has words to show: a plan line left empty once the direct answer and the
    /// receipts drawn as cards are taken out draws no bubble (owner 2026-11-06: "i get these emptyu bbubbles eveytime
    /// on a timer request").
    public var firstReplySays: Bool {
        func says(_ text: String) -> Bool { !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
        if let line = shownPlanLine {
            let receipts = CardLayout.remaining(receipts ?? [], receiptCards ?? [])
            return (shownDirect + receipts + [line] + (planLines ?? [])).contains(where: says)
        }
        if let plan = shownPlan { return says(plan) }
        return says(streamingPlan ?? "")
    }

    private func withoutDirect(_ text: String) -> String {
        guard let card else { return text }
        let answers = (direct ?? []) + card.rows.filter(\.direct).compactMap(\.text)
        var shown = text
        for answer in Set(answers) { shown = Self.withoutWhole(shown, answer) }
        return shown
    }

    /// `text` without each paragraph or line that is exactly `piece` (ignoring the whitespace around it), and without
    /// the blank lines that leaves; never a match inside other words ("4" leaves "compare the 4 laptops" alone,
    /// ORCH-549). The daemon's rule (supervisor.without_whole), for live and restored turns alike.
    public static func withoutWhole(_ text: String, _ piece: String) -> String {
        let piece = piece.trimmingCharacters(in: .whitespacesAndNewlines)
        func trimmed(_ part: Substring) -> String { part.trimmingCharacters(in: .whitespacesAndNewlines) }
        let paragraphs = text.components(separatedBy: "\n\n")
        let lines = text.split(separator: "\n", omittingEmptySubsequences: false)
        guard !piece.isEmpty, lines.contains(where: { trimmed($0) == piece })
                || paragraphs.contains(where: { trimmed(Substring($0)) == piece }) else { return text }
        var shown = paragraphs.filter { trimmed(Substring($0)) != piece }
            .map { $0.split(separator: "\n", omittingEmptySubsequences: false).filter { trimmed($0) != piece }
                .joined(separator: "\n") }
            .joined(separator: "\n\n")
        while shown.contains("\n\n\n") { shown = shown.replacingOccurrences(of: "\n\n\n", with: "\n\n") }
        return shown.trimmingCharacters(in: .whitespacesAndNewlines)
    }
    public var isRecovered: Bool { taskGoal != nil }
    public var isVerified: Bool { verified == true }
    /// What a report about this answer names (L35): its task, or its request when a planning failure ended the turn
    /// before any task existed.
    public var reportID: String? { backendTaskID ?? requestID }
    /// Whether the answer offers Send to developer (L35), by the rule the daemon's `reports.failure` applies: FERB
    /// recorded that the turn ended badly (its task ended failed or interrupted, or its turn failed: the reply or
    /// error says so). A status the notch inferred itself (`statusInferred`; a draft cut off before FERB confirmed it,
    /// `interrupted` with no task, saved before that flag existed) is no such record.
    public var offersReport: Bool {
        guard reportID != nil, statusInferred != true else { return false }
        return status == "failed" || (status == "interrupted" && backendTaskID != nil)
    }
    /// FERB's lines above the answer, in the order shown: the acknowledgement, the plan line, then each told part.
    public var ferbLines: [String] { [ack, plan].compactMap { $0 } + (parts ?? []) }
    /// The request as one line, for a late reply's quote.
    public var quote: String { (taskGoal ?? request).split(whereSeparator: \.isWhitespace).joined(separator: " ") }

    enum CodingKeys: String, CodingKey {
        case id, createdAt, request, sources, backendTaskID, answer, displayText, status, artifacts, responseNotice
        case requestID, ack, plan, parts, verified, question, taskGoal, taskName, statusInferred
        case planLine, planLines, direct, receipts, card, note, codeLines, runtimeSwitch, mergedInto
        case cards, receiptCards, hiddenKinds
    }
}

extension ConversationTurn {
    /// The daemon's name for one of the owner's own saved secrets (`Redactor.kinds`).
    static let savedSecret = "saved secret"

    /// Code's notice under the owner's message when FERB hid a secret in it (owner, 2026-11-01): a reminder, nothing
    /// to change, since the value never left the Mac.
    public var hiddenNotice: String? {
        let kinds = hiddenKinds ?? []
        let formats = kinds.filter { $0 != Self.savedSecret }
        var lines: [String] = []
        if kinds.contains(Self.savedSecret) { lines.append("That's one of your saved secrets. I hid it from the AI.") }
        if !formats.isEmpty {
            let them = formats.count == 1 ? "it" : "them"
            lines.append("That looked like \(Self.named(formats)). I hid \(them) from the AI and didn't save \(them). "
                         + "Best not to paste keys and passwords into chats.")
        }
        return lines.isEmpty ? nil : lines.joined(separator: " ")
    }

    /// Kinds as a sentence says them, as the daemon's `named_kinds` does: each with its article, joined with "and". An
    /// acronym (all capitals, or no vowel: "AWS", "npm") takes the article its first letter's name does.
    static func named(_ kinds: [String]) -> String {
        let said = kinds.map { kind -> String in
            let word = kind.split(separator: " ").first.map(String.init) ?? kind
            let acronym = word == word.uppercased() || !word.contains(where: { "aeiouAEIOU".contains($0) })
            let first = word.prefix(1).uppercased()
            return ((acronym ? "AEFHILMNORSX" : "AEIO").contains(first) && !first.isEmpty ? "an " : "a ") + kind
        }
        guard said.count > 2 else { return said.joined(separator: " and ") }
        return said.dropLast().joined(separator: ", ") + " and " + said[said.count - 1]
    }
}

/// The card row an open question is shown in.
public struct RowPlace: Equatable {
    public let turnID: UUID
    public let part: String
}

/// The current step of a running task, as the daemon describes it from the tool and its target.
public struct LiveProgress: Equatable {
    public let text: String
    public let startedAt: Date

    public init(text: String, startedAt: Date) {
        self.text = text
        self.startedAt = startedAt
    }

    /// Elapsed time since `startedAt`: "12s", "3m 05s", "1h 02m". Clock skew never shows negative time.
    public func elapsedText(at now: Date) -> String {
        let seconds = max(0, Int(now.timeIntervalSince(startedAt)))
        if seconds < 60 { return "\(seconds)s" }
        if seconds < 3600 { return String(format: "%dm %02ds", seconds / 60, seconds % 60) }
        return String(format: "%dh %02dm", seconds / 3600, seconds % 3600 / 60)
    }

    /// Reads the daemon's ISO 8601 timestamps (`at`, `started_at`): offset required, fractional seconds optional.
    public static func time(_ value: Any?) -> Date? {
        guard let text = value as? String, !text.isEmpty else { return nil }
        return try? Date.ISO8601FormatStyle(includingFractionalSeconds: true).parse(text)
    }
}

/// A late answer (L6): it arrived after later entries, so it is shown at the bottom with a one-line quote of its
/// question. The answer itself stays on its turn, so History keeps both entries and one copy of the text.
public struct ConversationReply: Identifiable, Codable, Equatable {
    public let id: UUID
    /// The turn this answers.
    public let turnID: UUID
    /// When the answer arrived; History files the reply under this day.
    public let createdAt: Date
    /// The newest turn when the answer arrived; the reply is shown after it.
    public let afterTurnID: UUID
    /// What it shows: FERB's question, or the answer. Replies saved before parts existed show the answer.
    public var part: ReplyPart? = nil
}

public enum ReplyPart: String, Codable {
    case question, answer
}

/// Outside text a notice quotes, with where it came from. It is data (MP2), shown as a quote.
public struct NoticeQuote: Codable, Equatable {
    public let text: String
    public let origin: String

    public init(text: String, origin: String) {
        self.text = text
        self.origin = origin
    }
}

/// Something FERB learned about the owner and asks them to confirm, with the owner's decision once made.
public struct LearningProposal: Codable, Equatable {
    public let id: String
    public let text: String
    /// "confirmed" or "not right" for a learning digest item; "kept", "dropped" or "edited" for a guess; nil while
    /// undecided.
    public var decision: String?
    /// "guess" for one of FERB's older guesses about the owner (L36: Keep, Edit or Drop); nil for a digest item.
    public var kind: String?

    public init(id: String, text: String, decision: String? = nil, kind: String? = nil) {
        self.id = id
        self.text = text
        self.decision = decision
        self.kind = kind
    }
}

/// A FERB message no turn asked for (L9): something FERB noticed, or what it learned and needs confirmed. It is shown
/// where it arrived, after the newest turn.
public struct ConversationNotice: Codable, Equatable {
    /// The daemon's id, so a notice is shown once however often it is delivered.
    public let id: String
    public let entryID: UUID
    /// When it reached this chat.
    public let createdAt: Date
    /// When the daemon noticed it (`at`), when known.
    public let noticedAt: Date?
    public let text: String
    public let quotes: [NoticeQuote]
    /// The small label shown with it ("noticed").
    public let tag: String?
    /// The newest turn when it arrived; nil when the chat had none.
    public let afterTurnID: UUID?
    /// The learning digest's proposals, each answered in place.
    public var proposals: [LearningProposal]?
    /// Typed cards from FERB's own records (step 12c: a memory import's examples as note cards and its page button).
    public var cards: [ResultCard]?
}

/// One entry of the chat as shown, top to bottom.
public enum TranscriptEntry: Identifiable, Equatable {
    case turn(ConversationTurn, answeredBelow: Bool)
    case reply(ConversationReply, answering: ConversationTurn)
    case notice(ConversationNotice)

    public var id: UUID {
        switch self {
        case let .turn(turn, _): return turn.id
        case let .reply(reply, _): return reply.id
        case let .notice(notice): return notice.entryID
        }
    }

    public var createdAt: Date {
        switch self {
        case let .turn(turn, _): return turn.createdAt
        case let .reply(reply, _): return reply.createdAt
        case let .notice(notice): return notice.createdAt
        }
    }
}

/// Calendar days are a presentation grouping; the daemon conversation remains continuous.
public struct ConversationArchive: Codable, Equatable {
    public private(set) var turns: [ConversationTurn] = []
    public private(set) var replies: [ConversationReply] = []
    public private(set) var notices: [ConversationNotice] = []
    public init() {}

    private enum CodingKeys: String, CodingKey { case turns, replies, notices }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        turns = try container.decode([ConversationTurn].self, forKey: .turns)
        // A message saved with the paste markers 0.27.1's ack echoed back reads as the owner wrote it (APP-712)
        for index in turns.indices { turns[index].request = PasteMarkers.removed(from: turns[index].request) }
        // Archives saved before late replies and notices existed have none.
        replies = try container.decodeIfPresent([ConversationReply].self, forKey: .replies) ?? []
        notices = try container.decodeIfPresent([ConversationNotice].self, forKey: .notices) ?? []
    }

    public func turns(on date: Date, calendar: Calendar = .current) -> [ConversationTurn] {
        turns.filter { calendar.isDate($0.createdAt, inSameDayAs: date) }
    }

    public var days: [Date] {
        let dates = turns.map(\.createdAt) + replies.map(\.createdAt) + notices.map(\.createdAt)
        return Array(Set(dates.map { Calendar.current.startOfDay(for: $0) })).sorted(by: >)
    }

    /// The chat in display order: each turn, followed by the late replies and notices that arrived while it was the
    /// newest, in arrival order. With a date, only the entries created that day (a reply counts on the day its answer
    /// arrived); that day is picked out first, so a long history isn't rebuilt on every read.
    public func transcript(on date: Date? = nil, calendar: Calendar = .current) -> [TranscriptEntry] {
        let day = date.flatMap { calendar.dateInterval(of: .day, for: $0) }
        let shown: (Date) -> Bool = { when in day.map { $0.start <= when && when < $0.end } ?? true }
        var following: [UUID?: [TranscriptEntry]] = [:]
        for (anchor, entry) in appended(where: shown) { following[anchor, default: []].append(entry) }
        let answeredBelow = Set(replies.map(\.turnID))
        var entries = following[nil] ?? []
        for turn in turns {
            if shown(turn.createdAt) { entries.append(.turn(turn, answeredBelow: answeredBelow.contains(turn.id))) }
            if let after = following[turn.id] { entries += after }
        }
        return entries
    }

    /// Late replies and notices accepted by `include`, each with the turn it follows, in arrival order.
    private func appended(where include: (Date) -> Bool) -> [(UUID?, TranscriptEntry)] {
        var items: [(Date, Int, UUID?, TranscriptEntry)] = []
        for reply in replies where include(reply.createdAt) {
            guard let answered = turns.first(where: { $0.id == reply.turnID }) else { continue }
            items.append((reply.createdAt, items.count, reply.afterTurnID, .reply(reply, answering: answered)))
        }
        for notice in notices where include(notice.createdAt) {
            items.append((notice.createdAt, items.count, notice.afterTurnID, .notice(notice)))
        }
        return items.sorted { ($0.0, $0.1) < ($1.0, $1.1) }.map { ($0.2, $0.3) }
    }

    /// The id of the entry shown last: the newest turn, or the latest reply or notice shown after it.
    private var lastEntryID: UUID? {
        guard let newest = turns.last else { return notices.last?.entryID }
        let reply = replies.last { $0.afterTurnID == newest.id }
        let notice = notices.last { $0.afterTurnID == newest.id }
        // In arrival order, as `transcript` shows them; at the same instant a notice follows a reply.
        if let reply, let notice { return notice.createdAt >= reply.createdAt ? notice.entryID : reply.id }
        return notice?.entryID ?? reply?.id ?? newest.id
    }

    /// The late replies shown for a turn, oldest first.
    public func repliesBelow(_ turnID: UUID) -> [ConversationReply] {
        replies.filter { $0.turnID == turnID }
    }

    /// `goal`: the turn is a task this chat didn't send, described by the daemon's goal.
    /// `unredacted`: `request` is raw typed text the daemon has not yet redacted; it stays out of every save until
    /// `acknowledge` brings the daemon's redacted message.
    public mutating func begin(id: UUID, request: String, sources: [URL], date: Date = Date(), goal: String? = nil,
                               unredacted: Bool = false) {
        guard !turns.contains(where: { $0.id == id }) else { return }
        var turn = ConversationTurn(id: id, createdAt: date, request: request, sources: sources, taskGoal: goal)
        turn.awaitingRedaction = unredacted
        turns.append(turn)
    }

    public static let unsavedPlaceholder = "Message not saved — sent before FERB confirmed it"

    /// The archive as it may be written to disk: a turn still awaiting the daemon's redaction saves a placeholder instead of its text.
    public func forStorage() -> ConversationArchive {
        var copy = self
        copy.turns.removeAll { $0.preview }
        for index in copy.turns.indices where copy.turns[index].awaitingRedaction { copy.turns[index].request = Self.unsavedPlaceholder }
        return copy
    }

    // MARK: The intro's preview (kb area app)

    /// A suggested task shown as the real chat shows a task, from `intro.preview`'s events (`plan`, a `part` per step,
    /// each marked `preview: true`), in a turn of its own marked preview. `running`: replayed as if under way, for
    /// practising Stop (its status `previewRunning` until `stopPreview`).
    public mutating func beginPreview(id: UUID, request: String, running: Bool = false, date: Date = Date()) {
        guard !turns.contains(where: { $0.id == id }) else { return }
        var turn = ConversationTurn(id: id, createdAt: date, request: request, sources: [])
        turn.preview = true
        turn.status = running ? "previewRunning" : "preview"
        turns.append(turn)
    }

    /// One preview event on its preview turn: the plan line and an empty card, then each step as a row. Events not
    /// marked preview are never applied here.
    public mutating func applyPreview(id: UUID, _ event: [String: Any]) {
        guard event["preview"] as? Bool == true,
              let index = turns.firstIndex(where: { $0.id == id && $0.preview }) else { return }
        let request = event["request_id"] as? String ?? ""
        switch event["type"] as? String {
        case "plan":
            var details = event
            details.removeValue(forKey: "task_id")
            showPlan(id: id, text: event["text"] as? String ?? "", details: details)
            if ((event["total"] as? NSNumber)?.intValue ?? 0) > 0 {
                turns[index].card = TurnCard(["request_id": request, "rows": [[String: Any]](), "total": event["total"] ?? 0],
                                             taskID: "preview:" + request)
            }
        case "part":
            guard let taskID = turns[index].card?.taskID else { return }
            var row = event
            row["task_id"] = taskID
            row["state"] = "preview"
            turns[index].card?.apply(part: row)
        default: break
        }
    }

    /// The replayed preview was stopped: it stays as a preview, no longer under way.
    public mutating func stopPreview(id: UUID) {
        guard let index = turns.firstIndex(where: { $0.id == id && $0.preview }) else { return }
        turns[index].status = "previewStopped"
    }

    /// The tour ended: its previews leave the chat.
    public mutating func removePreviews() {
        turns.removeAll { $0.preview }
    }

    /// Posts a FERB message no turn asked for, once per daemon id. Returns whether it is new to this chat.
    @discardableResult
    public mutating func addNotice(id: String, text: String, quotes: [NoticeQuote] = [], tag: String? = "noticed",
                                   noticedAt: Date? = nil, proposals: [LearningProposal]? = nil, cards: [ResultCard]? = nil,
                                   at date: Date = Date()) -> Bool {
        guard !id.isEmpty, !text.isEmpty, !notices.contains(where: { $0.id == id }) else { return false }
        notices.append(ConversationNotice(id: id, entryID: UUID(), createdAt: date, noticedAt: noticedAt, text: text,
                                          quotes: quotes, tag: tag, afterTurnID: turns.last?.id, proposals: proposals,
                                          cards: cards))
        return true
    }

    /// Records the owner's decision on one proposal of a learning digest.
    public mutating func decide(noticeID: String, proposalID: String, decision: String) {
        guard let index = notices.firstIndex(where: { $0.id == noticeID }),
              let item = notices[index].proposals?.firstIndex(where: { $0.id == proposalID }) else { return }
        notices[index].proposals?[item].decision = decision
    }

    /// A finished task this chat didn't send (started elsewhere, or before a restart), filed among the turns at the
    /// time its result arrived and answered in place: it is FERB's message, not a late answer to the owner.
    public mutating func recordRecovered(taskID: String, goal: String, answer: String, status: String, artifacts: [URL],
                                         displayText: String?, verified: Bool, at date: Date) {
        guard !turns.contains(where: { $0.backendTaskID == taskID }) else { return }
        var turn = ConversationTurn(id: UUID(), createdAt: date, request: "", sources: [], backendTaskID: taskID,
                                    taskGoal: goal)
        turn.answer = answer
        turn.displayText = displayText
        turn.status = status
        turn.artifacts = artifacts
        turn.verified = verified
        turns.insert(turn, at: turns.firstIndex { $0.createdAt > date } ?? turns.endIndex)
    }

    public mutating func bind(id: UUID, taskID: String) {
        guard let index = turns.firstIndex(where: { $0.id == id }) else { return }
        turns[index].backendTaskID = taskID
    }

    /// Ties the daemon's `request_id` to a turn. A turn belongs to one request and a request to one turn: this
    /// returns false when the event names another turn's request, and that event must not be applied here.
    public mutating func bindRequest(id: UUID, requestID: String) -> Bool {
        if let owner = turns.first(where: { $0.requestID == requestID }) { return owner.id == id }
        guard let index = turns.firstIndex(where: { $0.id == id }) else { return true }
        guard turns[index].requestID == nil else { return false }
        turns[index].requestID = requestID
        return true
    }

    public func turn(requestID: String) -> ConversationTurn? {
        turns.first { $0.requestID == requestID }
    }

    /// `message`: the owner's message as the daemon redacted it (L7); it replaces the raw text the turn was sent with.
    /// Without one the raw text stays in memory only. `hidden`: the kinds of secret hidden in it (`hiddenNotice`).
    public mutating func acknowledge(id: UUID, text: String, message: String? = nil, hidden: [String]? = nil) {
        guard let index = turns.firstIndex(where: { $0.id == id }) else { return }
        if let message, !message.isEmpty {
            // As the daemon records it; paste markers an older daemon echoed back never show (APP-712)
            turns[index].request = PasteMarkers.removed(from: message)
            turns[index].awaitingRedaction = false
        }
        if let hidden, !hidden.isEmpty { turns[index].hiddenKinds = hidden }
        if turns[index].status == "working" { turns[index].awaitingReply = true }
        guard !text.isEmpty else { return }
        turns[index].ack = text
    }

    /// The plan event (L7): its text, and its fields when the daemon sends them (`details`, ORCH-319): the plan line
    /// and code's lines, the asks answered at once, the receipts and the card.
    public mutating func showPlan(id: UUID, text: String, details: [String: Any] = [:]) {
        guard let index = turns.firstIndex(where: { $0.id == id }) else { return }
        turns[index].streamingPlan = nil
        turns[index].awaitingReply = false
        if !text.isEmpty { turns[index].plan = text }
        if details["name"] != nil || details["goal"] != nil {
            let name = ShortName.of(details, fallback: "goal")
            if !name.isEmpty { turns[index].taskName = name }
        }
        if let line = details["plan_line"] as? String {
            turns[index].planLine = line
            turns[index].planLines = (details["lines"] as? [String] ?? []).filter { !$0.isEmpty }
        }
        if let direct = details["direct"] as? [[String: Any]] {
            turns[index].direct = direct.compactMap { $0["text"] as? String }
                .filter { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
        }
        if let receipts = details["receipts"] as? [String] { turns[index].receipts = receipts.filter { !$0.isEmpty } }
        if let cards = ResultCard.list(details["cards"]) { turns[index].receiptCards = cards }
        if let taskID = details["task_id"] as? String, let card = TurnCard(details["card"], taskID: taskID) {
            place(card, at: index)
        }
    }

    /// The plan line as it streams (`plan_delta`); the plan event replaces it.
    public mutating func streamPlan(id: UUID, delta: String) {
        guard !delta.isEmpty, let index = turns.firstIndex(where: { $0.id == id && $0.status == "working" }) else { return }
        turns[index].streamingPlan = (turns[index].streamingPlan ?? "") + delta
        turns[index].awaitingReply = false
    }

    public mutating func abortPlanStream(id: UUID) {
        guard let index = turns.firstIndex(where: { $0.id == id }) else { return }
        turns[index].streamingPlan = nil
    }

    /// A newer picture of a turn's card merges into the one shown; another turn's card replaces it.
    private mutating func place(_ card: TurnCard, at index: Int) {
        if var shown = turns[index].card, shown.requestID == card.requestID, shown.taskID == card.taskID {
            shown.merge(card)
            turns[index].card = shown
        } else {
            turns[index].card = card
        }
    }

    /// A row's change (`part`, on the turn's stream and on the bus as `task.part`), applied to the row keyed by its
    /// task and part in whichever card shows it: this run's card, else the card that first showed the part, else the
    /// newest card with that part; a part no card shows yet joins this run's card. Returns the turn that changed.
    @discardableResult
    public mutating func applyPart(_ event: [String: Any]) -> UUID? {
        guard let taskID = event["task_id"] as? String, let part = event["part"] as? String else { return nil }
        let request = event["request_id"] as? String
        let first = event["card_request_id"] as? String
        func showing(_ requestID: String?) -> Int? {
            guard let requestID else { return nil }
            return turns.lastIndex { $0.card?.taskID == taskID && $0.card?.requestID == requestID && $0.card?.row(part) != nil }
        }
        let index = showing(request) ?? showing(first)
            ?? turns.lastIndex { $0.card?.taskID == taskID && $0.card?.row(part) != nil }
            ?? turns.lastIndex { $0.card?.taskID == taskID && $0.card?.requestID == request }
        guard let index, turns[index].card?.apply(part: event) == true else { return nil }
        return turns[index].id
    }

    /// A step's state (`step`, on the turn's stream and on the bus as `task.step`, ORCH-531), applied to the card of its
    /// task and turn, else the task's newest card. Live state: nothing to save. Returns the turn that changed.
    @discardableResult
    public mutating func applyStep(_ event: [String: Any]) -> UUID? {
        guard let taskID = event["task_id"] as? String else { return nil }
        let request = event["request_id"] as? String
        guard let index = turns.lastIndex(where: { $0.card?.taskID == taskID && $0.card?.requestID == request })
                ?? turns.lastIndex(where: { $0.card?.taskID == taskID }),
              turns[index].card?.apply(step: event) == true else { return nil }
        return turns[index].id
    }

    /// A card the daemon reports for a running or resumed task (`chat.status`, a resumed task's plan): merged into the
    /// turn showing it, else given to the turn of its request or to the task's newest turn without one; a task this
    /// chat has no turn for gets one, described by its goal. Returns whether anything changed.
    @discardableResult
    public mutating func showCard(_ card: TurnCard, goal: String, at date: Date = Date()) -> Bool {
        let before = turns
        if let index = turns.lastIndex(where: { $0.card?.requestID == card.requestID && $0.card?.taskID == card.taskID })
            ?? turns.lastIndex(where: { !card.requestID.isEmpty && $0.requestID == card.requestID })
            ?? turns.lastIndex(where: { $0.backendTaskID == card.taskID && $0.card == nil }) {
            if turns[index].backendTaskID == nil { turns[index].backendTaskID = card.taskID }
            place(card, at: index)
        } else {
            var turn = ConversationTurn(id: UUID(), createdAt: date, request: "", sources: [], backendTaskID: card.taskID,
                                        taskGoal: goal)
            turn.requestID = card.requestID.isEmpty ? nil : card.requestID
            turn.card = card
            turns.append(turn)
        }
        return turns != before
    }

    /// The outcome's own fields (ORCH-319) on the turn that asked (its request, else the task's newest turn): a
    /// one-ask answer's "Couldn't confirm" and code lines; a multi-part task's footer; the offer of another
    /// AI service. Returns whether the turn changed.
    @discardableResult
    public mutating func applyOutcome(_ event: [String: Any]) -> Bool {
        guard let taskID = event["task_id"] as? String else { return false }
        let request = event["request_id"] as? String
        guard let index = turns.lastIndex(where: { request != nil && $0.requestID == request })
            ?? turns.lastIndex(where: { $0.backendTaskID == taskID }) else { return false }
        let before = turns[index]
        func words(_ key: String) -> String? { (event[key] as? String).flatMap { $0.isEmpty ? nil : $0 } }
        turns[index].note = words("note")
        turns[index].codeLines = (event["lines"] as? [String]).flatMap { $0.isEmpty ? nil : $0 }
        turns[index].runtimeSwitch = RuntimeSwitch(event["runtime_switch"])
        if let card = TurnCard(event["card"], taskID: taskID) { place(card, at: index) }
        if let cards = ResultCard.list(event["cards"]) { turns[index].cards = keepingSettled(cards, turns[index].cards) }
        return turns[index] != before
    }

    /// A reply's cards (step 8e: a remembered fact, a reminder or schedule just set).
    public mutating func showCards(id: UUID, _ value: Any?) {
        guard let cards = ResultCard.list(value), let index = turns.firstIndex(where: { $0.id == id }) else { return }
        turns[index].cards = keepingSettled(cards, turns[index].cards)
    }

    /// A note card the owner corrected or forgot here keeps saying so when the same cards arrive again.
    private func keepingSettled(_ cards: [ResultCard], _ shown: [ResultCard]?) -> [ResultCard] {
        cards.map { card in
            guard card.type == "note", let settled = shown?.first(where: {
                $0.type == "note" && $0.itemID == card.itemID && ["corrected", "forgotten"].contains($0.state ?? "")
            }) else { return card }
            return settled
        }
    }

    /// The owner corrected (`text`) or forgot (`forgotten`) a remembered item from its note card: every card of that
    /// item says so, in History too.
    public mutating func settleNote(itemID: String, text: String? = nil, forgotten: Bool = false) {
        for index in turns.indices {
            for list in [\ConversationTurn.cards, \ConversationTurn.receiptCards] {
                guard var cards = turns[index][keyPath: list] else { continue }
                var changed = false
                for i in cards.indices where cards[i].type == "note" && cards[i].itemID == itemID {
                    if let text { cards[i].text = text }
                    cards[i].state = forgotten ? "forgotten" : "corrected"
                    changed = true
                }
                if changed { turns[index][keyPath: list] = cards }
            }
        }
    }

    /// A failed reply's offer of another AI service (ORCH-315).
    public mutating func offerRuntimeSwitch(id: UUID, _ offer: RuntimeSwitch?) {
        guard let offer, let index = turns.firstIndex(where: { $0.id == id }) else { return }
        turns[index].runtimeSwitch = offer
    }

    /// The message was read together with another one (`merged`, ORCH-318): that message's reply answers it.
    public mutating func merge(id: UUID, into requestID: String) {
        guard let index = turns.firstIndex(where: { $0.id == id && $0.status == "working" && $0.backendTaskID == nil }) else { return }
        turns[index].mergedInto = requestID
        turns[index].status = "merged"
        turns[index].awaitingReply = false
        turns[index].streamingPlan = nil
        turns[index].liveLine = nil
    }

    /// Where each open question is shown in a card (APP-300: a question with its buttons inside its row): the
    /// newest turn whose card has a row it belongs to. A question no row claims stays at the end of the chat.
    public func rowPlacements(_ approvals: [DaemonApproval]) -> [String: RowPlace] {
        var placed: [String: RowPlace] = [:]
        let carded = turns.filter { $0.card != nil }.reversed()
        for approval in approvals {
            for turn in carded {
                if let row = turn.card?.rows.first(where: { $0.asks(approval) }) {
                    placed[approval.id] = RowPlace(turnID: turn.id, part: row.part)
                    break
                }
            }
        }
        return placed
    }

    /// Replaces the turn's one live line while it waits for its answer.
    public mutating func showProgress(id: UUID, text: String, startedAt: Date) {
        guard !text.isEmpty, let index = turns.firstIndex(where: { $0.id == id && $0.status == "working" }) else { return }
        turns[index].liveLine = LiveProgress(text: text, startedAt: startedAt)
    }

    /// The live line describes a stream that is still reporting; without one it would show a stale step.
    public mutating func clearLiveLine(id: UUID) {
        guard let index = turns.firstIndex(where: { $0.id == id }) else { return }
        turns[index].liveLine = nil
    }

    public mutating func append(id: UUID, delta: String) {
        guard let index = turns.firstIndex(where: { $0.id == id && $0.status == "working" && $0.backendTaskID == nil }) else { return }
        turns[index].answer += delta
    }

    public mutating func clearProvisionalReply(id: UUID) {
        guard let index = turns.firstIndex(where: { $0.id == id && ["working", "interrupted"].contains($0.status) }) else { return }
        turns[index].answer = ""
        turns[index].responseNotice = nil
        turns[index].status = "working"
        turns[index].statusInferred = nil
    }

    public mutating func abortReply(id: UUID, reason: String) {
        guard let index = turns.firstIndex(where: { $0.id == id && $0.status == "working" && $0.backendTaskID == nil }) else { return }
        turns[index].answer = ""
        turns[index].responseNotice = reason.isEmpty ? "The draft was interrupted. Waiting for FERB’s final response." : reason
        turns[index].status = "interrupted"
        turns[index].statusInferred = true
        turns[index].liveLine = nil
    }

    public mutating func interruptRestoredDrafts() {
        for id in turns.filter({ $0.status == "working" && $0.backendTaskID == nil }).map(\.id) {
            abortReply(id: id, reason: "FERB closed before this draft was confirmed.")
        }
    }

    /// Records a turn's answer and removes its live line. A turn's first answer that arrives after later entries
    /// is shown at the bottom as a reply quoting the question (L6); the turn itself then reads "answered below".
    /// The final answer to a question FERB asked (`needs_user`) is placed the same way, and the question stays where
    /// it was shown. `inferred`: the notch inferred `status` itself, with no answer from FERB (`statusInferred`).
    public mutating func finish(id: UUID, answer: String, status: String, artifacts: [URL], displayText: String? = nil,
                                verified: Bool = false, inferred: Bool = false, at date: Date = Date()) {
        guard let index = turns.firstIndex(where: { $0.id == id }) else { return }
        let before = turns[index].status
        let firstAnswer = Self.awaitingAnswer.contains(before) && !Self.awaitingAnswer.contains(status)
        let afterQuestion = before == "needs_user" && status != "needs_user" && !Self.awaitingAnswer.contains(status)
        if afterQuestion { turns[index].question = turns[index].visibleAnswer }
        turns[index].answer = answer
        turns[index].displayText = displayText
        turns[index].status = status
        turns[index].artifacts = artifacts
        turns[index].responseNotice = nil
        turns[index].verified = verified
        turns[index].statusInferred = inferred ? true : nil
        turns[index].liveLine = nil
        // The final answer replaces the turn's streamed draft whichever channel it streamed on: a steer's reply
        // streams as a plan line (plan_delta, plan_end) and ends as `reply`, with no plan event to clear it (APP-714)
        turns[index].streamingPlan = nil
        // A card fills in place (ORCH-319): its rows and footer are the answer, never a reply further down.
        guard firstAnswer || afterQuestion, turns[index].card == nil, let newest = turns.last else { return }
        if firstAnswer && replies.contains(where: { $0.turnID == id }) { return }
        // Its words so far are shown on the turn, or in the reply below it that showed the question.
        let shownAt = replies.last(where: { $0.turnID == id })?.id ?? id
        guard shownAt != lastEntryID else { return }   // nothing is shown below it: answered in place
        replies.append(ConversationReply(id: UUID(), turnID: id, createdAt: date, afterTurnID: newest.id,
                                         part: status == "needs_user" ? .question : .answer))
    }

    /// Statuses of a turn that has no answer yet.
    private static let awaitingAnswer: Set<String> = ["working", "interrupted"]
}
