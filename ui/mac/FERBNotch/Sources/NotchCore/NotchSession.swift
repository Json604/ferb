import Foundation

public enum SessionPhase: String, Equatable {
    case ready, listening, draft, working, result, stopped, attention
}

public struct TaskActivity: Identifiable, Equatable {
    public let id = UUID()
    public let title: String
    public let symbol: String
    public let createdAt = Date()
}

public struct WorkReceipt: Identifiable, Equatable {
    public let id: UUID
    public let request: String
    public let sources: [URL]
    public let createdAt: Date
    public let activity: [TaskActivity]
    public let documentURL: URL?
    public var backendTaskID: String? = nil
    public var answer: String = ""
    public var status: String = "done"
    public var artifacts: [URL] = []
}

/// Visibility is independent of execution. Hiding a surface cannot cancel work.
public struct NotchSession {
    public var isExpanded = false
    public var draft = ""
    public private(set) var phase: SessionPhase = .ready
    public private(set) var attachments: [URL] = []
    public private(set) var activeTaskID: UUID?
    public private(set) var backendTaskID: String?
    public private(set) var result: WorkReceipt?
    public private(set) var history: [WorkReceipt] = []
    public private(set) var activity: [TaskActivity] = []
    public private(set) var startedAt: Date?
    public private(set) var attentionMessage: String?
    private var submittedRequest = ""
    private var submittedSources: [URL] = []

    public init() {}
    public mutating func collapse() { isExpanded = false }
    public mutating func expand() { isExpanded = true }

    public mutating func edit() {
        guard phase != .working else { return }
        phase = .draft
        isExpanded = true
    }

    public mutating func beginVoicePreview() {
        guard phase != .working else { return }
        phase = .listening
        isExpanded = true
    }

    public mutating func finishVoicePreview() {
        guard phase == .listening else { return }
        phase = .draft
    }

    public mutating func attach(_ urls: [URL]) {
        for url in urls where url.isFileURL {
            let normalized = url.standardizedFileURL
            if !attachments.contains(normalized) { attachments.append(normalized) }
        }
    }

    public mutating func removeAttachment(_ url: URL) {
        attachments.removeAll { $0 == url }
    }

    @discardableResult
    public mutating func beginWork() -> UUID? {
        guard phase != .working, !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
        let id = UUID()
        activeTaskID = id
        backendTaskID = nil
        startedAt = Date()
        activity = [TaskActivity(title: "Request received", symbol: "checkmark")]
        submittedRequest = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        submittedSources = attachments
        result = nil
        attentionMessage = nil
        phase = .working
        return id
    }

    public mutating func recordActivity(taskID: UUID, title: String, symbol: String) {
        guard activeTaskID == taskID, phase == .working else { return }
        activity.append(TaskActivity(title: title, symbol: symbol))
    }

    @discardableResult
    public mutating func finishWork(taskID: UUID, documentURL: URL? = nil) -> Bool {
        guard phase == .working, activeTaskID == taskID else { return false }
        activity.append(TaskActivity(title: "Work finished", symbol: "checkmark"))
        let receipt = WorkReceipt(id: taskID, request: submittedRequest,
                                  sources: submittedSources, createdAt: Date(), activity: activity,
                                  documentURL: documentURL)
        result = receipt
        history.insert(receipt, at: 0)
        activeTaskID = nil
        phase = .result
        return true
    }

    public mutating func stopWork() {
        guard phase == .working else { return }
        activeTaskID = nil
        activity.append(TaskActivity(title: "Stopped by you", symbol: "pause"))
        phase = .stopped
    }

    public mutating func requireAttention(_ message: String) {
        guard phase == .working else { return }
        activeTaskID = nil
        attentionMessage = message
        activity.append(TaskActivity(title: message, symbol: "exclamationmark.triangle"))
        phase = .attention
    }

    public mutating func clearComposer() {
        draft = ""
        attachments = []
    }

    public mutating func newRequest() {
        guard phase != .working else { return }
        draft = ""
        activeTaskID = nil
        backendTaskID = nil
        attachments = []
        result = nil
        activity = []
        startedAt = nil
        attentionMessage = nil
        phase = .ready
        isExpanded = true
    }

    public mutating func bindBackendTask(_ taskID: String, localID: UUID) {
        guard activeTaskID == localID else { return }
        backendTaskID = taskID
    }

    public mutating func resumeBackendTask(_ taskID: String, goal: String) {
        guard backendTaskID != taskID || activeTaskID == nil else { return }
        backendTaskID = taskID
        activeTaskID = UUID()
        submittedRequest = goal
        submittedSources = []
        result = nil
        startedAt = Date()
        activity = [TaskActivity(title: "Reconnected to running task", symbol: "arrow.triangle.2.circlepath")]
        attentionMessage = nil
        phase = .working
    }

    public mutating func detachInterruptedReply(_ message: String) {
        guard backendTaskID == nil, activeTaskID != nil else { return }
        activeTaskID = nil
        attentionMessage = message
        phase = .attention
    }

    public mutating func detachUnavailableTask(_ message: String) {
        activeTaskID = nil
        attentionMessage = message
        phase = .attention
    }

    public mutating func waitForOwner(_ message: String) {
        attentionMessage = message
        phase = .attention
    }

    public mutating func resumeAfterOwnerAnswer() {
        guard activeTaskID != nil else { return }
        attentionMessage = nil
        phase = .working
    }

    public mutating func noteBackendActivity(_ text: String, symbol: String = "ellipsis") {
        guard activeTaskID != nil, activity.last?.title != text else { return }
        activity.append(TaskActivity(title: text, symbol: symbol))
    }

    @discardableResult
    public mutating func finishBackendWork(taskID: String?, status: String, answer: String, artifacts: [String]) -> Bool {
        if let taskID, backendTaskID != taskID { return false }
        guard let localID = activeTaskID else { return false }
        let urls = artifacts.filter { $0.hasPrefix("/") }.map { URL(fileURLWithPath: $0) }
        let receipt = WorkReceipt(id: localID, request: submittedRequest, sources: submittedSources,
            createdAt: Date(), activity: activity, documentURL: urls.first, backendTaskID: taskID,
            answer: answer, status: status, artifacts: urls)
        result = receipt
        if let taskID { history.removeAll { $0.backendTaskID == taskID } }
        history.insert(receipt, at: 0)
        activeTaskID = nil
        attentionMessage = status == "done" || status == "cancelled" ? nil : answer
        phase = status == "done" ? .result : (status == "cancelled" ? .stopped : .attention)
        return true
    }

    public mutating func recall(_ receipt: WorkReceipt) {
        guard phase != .working else { return }
        result = receipt
        backendTaskID = receipt.backendTaskID
        draft = receipt.request
        attachments = receipt.sources
        activity = receipt.activity
        phase = .result
        isExpanded = true
    }
}
