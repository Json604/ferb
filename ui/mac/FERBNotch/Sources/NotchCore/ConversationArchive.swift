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
}

/// Calendar days are a presentation grouping; the daemon conversation remains continuous.
public struct ConversationArchive: Codable, Equatable {
    public private(set) var turns: [ConversationTurn] = []
    public init() {}

    public func turns(on date: Date, calendar: Calendar = .current) -> [ConversationTurn] {
        turns.filter { calendar.isDate($0.createdAt, inSameDayAs: date) }
    }

    public var days: [Date] {
        Array(Set(turns.map { Calendar.current.startOfDay(for: $0.createdAt) })).sorted(by: >)
    }

    public mutating func begin(id: UUID, request: String, sources: [URL], date: Date = Date()) {
        guard !turns.contains(where: { $0.id == id }) else { return }
        turns.append(ConversationTurn(id: id, createdAt: date, request: request, sources: sources))
    }

    public mutating func bind(id: UUID, taskID: String) {
        guard let index = turns.firstIndex(where: { $0.id == id }) else { return }
        turns[index].backendTaskID = taskID
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
    }

    public mutating func interruptRestoredDrafts() {
        for id in turns.filter({ $0.status == "working" && $0.backendTaskID == nil }).map(\.id) {
            abortReply(id: id, reason: "FERB closed before this draft was confirmed.")
        }
    }

    public mutating func finish(id: UUID, answer: String, status: String, artifacts: [URL], displayText: String? = nil) {
        guard let index = turns.firstIndex(where: { $0.id == id }) else { return }
        turns[index].answer = answer
        turns[index].displayText = displayText
        turns[index].status = status
        turns[index].artifacts = artifacts
        turns[index].responseNotice = nil
    }
}
