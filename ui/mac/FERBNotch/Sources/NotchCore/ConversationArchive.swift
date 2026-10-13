import Foundation

public struct ConversationTurn: Identifiable, Codable, Equatable {
    public let id: UUID
    public let createdAt: Date
    /// The message the owner sent. It is saved with the conversation (History shows both sides); the unsent
    /// composer draft never is (L20).
    public let request: String
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
    /// The outcome's `verified` flag, shown as a small check mark.
    public var verified: Bool?
    /// The one live step line under the plan line (`progress`, L8). Live state only: never saved.
    public var liveLine: LiveProgress? = nil
    /// The question FERB asked (a `needs_user` answer), kept where it was shown once the final answer arrives.
    public var question: String?
    /// The daemon's goal for a task this chat didn't send (reconnected, recovered or started elsewhere); empty when
    /// unknown. Such a turn has no owner message: the goal is FERB's description, never words the owner typed.
    public var taskGoal: String?

    public var isRecovered: Bool { taskGoal != nil }
    public var isVerified: Bool { verified == true }
    /// FERB's lines above the answer, in the order shown: the acknowledgement, then the plan line.
    public var ferbLines: [String] { [ack, plan].compactMap { $0 } }
    /// The request as one line, for a late reply's quote.
    public var quote: String { (taskGoal ?? request).split(whereSeparator: \.isWhitespace).joined(separator: " ") }

    enum CodingKeys: String, CodingKey {
        case id, createdAt, request, sources, backendTaskID, answer, displayText, status, artifacts, responseNotice
        case requestID, ack, plan, verified, question, taskGoal
    }
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
    /// "confirmed" or "not right"; nil while undecided.
    public var decision: String?

    public init(id: String, text: String, decision: String? = nil) {
        self.id = id
        self.text = text
        self.decision = decision
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
    public mutating func begin(id: UUID, request: String, sources: [URL], date: Date = Date(), goal: String? = nil) {
        guard !turns.contains(where: { $0.id == id }) else { return }
        turns.append(ConversationTurn(id: id, createdAt: date, request: request, sources: sources, taskGoal: goal))
    }

    /// Posts a FERB message no turn asked for, once per daemon id. Returns whether it is new to this chat.
    @discardableResult
    public mutating func addNotice(id: String, text: String, quotes: [NoticeQuote] = [], tag: String? = "noticed",
                                   noticedAt: Date? = nil, proposals: [LearningProposal]? = nil, at date: Date = Date()) -> Bool {
        guard !id.isEmpty, !text.isEmpty, !notices.contains(where: { $0.id == id }) else { return false }
        notices.append(ConversationNotice(id: id, entryID: UUID(), createdAt: date, noticedAt: noticedAt, text: text,
                                          quotes: quotes, tag: tag, afterTurnID: turns.last?.id, proposals: proposals))
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

    public mutating func acknowledge(id: UUID, text: String) {
        guard !text.isEmpty, let index = turns.firstIndex(where: { $0.id == id }) else { return }
        turns[index].ack = text
    }

    public mutating func showPlan(id: UUID, text: String) {
        guard !text.isEmpty, let index = turns.firstIndex(where: { $0.id == id }) else { return }
        turns[index].plan = text
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
    }

    public mutating func abortReply(id: UUID, reason: String) {
        guard let index = turns.firstIndex(where: { $0.id == id && $0.status == "working" && $0.backendTaskID == nil }) else { return }
        turns[index].answer = ""
        turns[index].responseNotice = reason.isEmpty ? "The draft was interrupted. Waiting for FERB’s final response." : reason
        turns[index].status = "interrupted"
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
    /// it was shown.
    public mutating func finish(id: UUID, answer: String, status: String, artifacts: [URL], displayText: String? = nil,
                                verified: Bool = false, at date: Date = Date()) {
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
        turns[index].liveLine = nil
        guard firstAnswer || afterQuestion, let newest = turns.last else { return }
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
