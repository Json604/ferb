import Foundation

public struct ConversationTurn: Identifiable, Codable, Equatable {
    public let id: UUID
    public let createdAt: Date
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

    public var isVerified: Bool { verified == true }
    /// FERB's lines above the answer, in the order shown: the acknowledgement, then the plan line.
    public var ferbLines: [String] { [ack, plan].compactMap { $0 } }
    /// The question as one line, for a late reply's quote.
    public var quote: String { request.split(whereSeparator: \.isWhitespace).joined(separator: " ") }

    enum CodingKeys: String, CodingKey {
        case id, createdAt, request, sources, backendTaskID, answer, displayText, status, artifacts, responseNotice
        case requestID, ack, plan, verified
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
}

/// One entry of the chat as shown, top to bottom.
public enum TranscriptEntry: Identifiable, Equatable {
    case turn(ConversationTurn, answeredBelow: Bool)
    case reply(ConversationReply, answering: ConversationTurn)

    public var id: UUID {
        switch self {
        case let .turn(turn, _): return turn.id
        case let .reply(reply, _): return reply.id
        }
    }

    public var createdAt: Date {
        switch self {
        case let .turn(turn, _): return turn.createdAt
        case let .reply(reply, _): return reply.createdAt
        }
    }
}

/// Calendar days are a presentation grouping; the daemon conversation remains continuous.
public struct ConversationArchive: Codable, Equatable {
    public private(set) var turns: [ConversationTurn] = []
    public private(set) var replies: [ConversationReply] = []
    public init() {}

    private enum CodingKeys: String, CodingKey { case turns, replies }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        turns = try container.decode([ConversationTurn].self, forKey: .turns)
        // Archives saved before late replies existed have none.
        replies = try container.decodeIfPresent([ConversationReply].self, forKey: .replies) ?? []
    }

    public func turns(on date: Date, calendar: Calendar = .current) -> [ConversationTurn] {
        turns.filter { calendar.isDate($0.createdAt, inSameDayAs: date) }
    }

    public var days: [Date] {
        let dates = turns.map(\.createdAt) + replies.map(\.createdAt)
        return Array(Set(dates.map { Calendar.current.startOfDay(for: $0) })).sorted(by: >)
    }

    /// The chat in display order: each turn, followed by the late replies that arrived while it was the newest.
    /// With a date, only the entries created that day (a reply counts on the day its answer arrived).
    public func transcript(on date: Date? = nil, calendar: Calendar = .current) -> [TranscriptEntry] {
        let turnsByID = Dictionary(turns.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        let answeredBelow = Set(replies.map(\.turnID))
        let repliesAfter = Dictionary(grouping: replies, by: \.afterTurnID)
        var entries: [TranscriptEntry] = []
        for turn in turns {
            entries.append(.turn(turn, answeredBelow: answeredBelow.contains(turn.id)))
            for reply in repliesAfter[turn.id] ?? [] {
                if let answered = turnsByID[reply.turnID] { entries.append(.reply(reply, answering: answered)) }
            }
        }
        guard let date else { return entries }
        return entries.filter { calendar.isDate($0.createdAt, inSameDayAs: date) }
    }

    public mutating func begin(id: UUID, request: String, sources: [URL], date: Date = Date()) {
        guard !turns.contains(where: { $0.id == id }) else { return }
        turns.append(ConversationTurn(id: id, createdAt: date, request: request, sources: sources))
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
    public mutating func finish(id: UUID, answer: String, status: String, artifacts: [URL], displayText: String? = nil,
                                verified: Bool = false, at date: Date = Date()) {
        guard let index = turns.firstIndex(where: { $0.id == id }) else { return }
        let firstAnswer = Self.awaitingAnswer.contains(turns[index].status) && !Self.awaitingAnswer.contains(status)
        turns[index].answer = answer
        turns[index].displayText = displayText
        turns[index].status = status
        turns[index].artifacts = artifacts
        turns[index].responseNotice = nil
        turns[index].verified = verified
        turns[index].liveLine = nil
        guard firstAnswer, !isLastEntry(id), !replies.contains(where: { $0.turnID == id }), let newest = turns.last else { return }
        replies.append(ConversationReply(id: UUID(), turnID: id, createdAt: date, afterTurnID: newest.id))
    }

    /// Statuses of a turn that has no answer yet.
    private static let awaitingAnswer: Set<String> = ["working", "interrupted"]

    /// An answer stays in place only when nothing is shown below its turn.
    private func isLastEntry(_ id: UUID) -> Bool {
        turns.last?.id == id && !replies.contains { $0.afterTurnID == id }
    }
}
