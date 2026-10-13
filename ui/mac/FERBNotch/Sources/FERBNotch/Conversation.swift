import Foundation
import NotchCore

@MainActor
extension NotchModel {
    var canCompose: Bool { session.phase != .listening && pendingApprovals.isEmpty }
    var hasRunningWork: Bool { activeTaskCount > 0 }
    var displayPhase: SessionPhase { !pendingApprovals.isEmpty ? .attention : (hasRunningWork && session.phase != .attention ? .working : session.phase) }
    var workStatus: String {
        let active = workSessions.values.filter { $0.activeTaskID != nil }
        if active.count > 1 { return "\(active.count) requests running" }
        return (session.activeTaskID != nil ? session : active.first)?.activity.last?.title ?? "Working…"
    }
    var visibleTurns: [ConversationTurn] { conversation.turns(on: followsToday ? Date() : selectedDay) }
    var viewingToday: Bool { followsToday || Calendar.current.isDateInToday(selectedDay) }

    /// Each stream owns a task session; the composer belongs to the window.
    func withWorkSession(_ id: UUID, action: () -> Void) {
        let foreground = session
        let attentionBefore = attentionStartedAt
        let isSelected = session.activeTaskID == id
        if !isSelected {
            guard let work = workSessions[id], work.activeTaskID != nil else { return }
            session = work
        }
        defer {
            if session.activeTaskID != nil { workSessions[id] = session }
            else { workSessions.removeValue(forKey: id) }
            if !isSelected { session = foreground }
            session.draft = foreground.draft
            // Presentation belongs to the window, never a background work snapshot.
            if foreground.isExpanded || attentionBefore != attentionStartedAt { session.expand() } else { session.collapse() }
            onBackgroundChange?(true)
        }
        action()
    }

    func saveConversation() {
        do { preferences.set(try JSONEncoder().encode(conversation), forKey: "FERBConversationArchive") }
        catch { promptError = "Conversation history could not be saved: " + error.localizedDescription }
    }

    func selectDay(_ day: Date) {
        followsToday = Calendar.current.isDateInToday(day)
        measuredTranscriptHeight = 0
        selectedDay = Calendar.current.startOfDay(for: day)
        showingHistory = false
        showingSources = false
        showingActivity = false
        onInteraction?()
        onPresentationChange?(true)
    }

    func recordAnswer(announce: Bool, displayText: String? = nil) {
        guard let receipt = session.result else { return }
        let existing = conversation.turns.first { turn in
            turn.id == receipt.id || (receipt.backendTaskID != nil && turn.backendTaskID == receipt.backendTaskID)
        }
        let id = existing?.id ?? receipt.id
        conversation.begin(id: id, request: receipt.request, sources: receipt.sources, date: receipt.createdAt)
        if let taskID = receipt.backendTaskID { conversation.bind(id: id, taskID: taskID) }
        conversation.finish(id: id, answer: receipt.answer, status: receipt.status, artifacts: receipt.artifacts, displayText: displayText)
        saveConversation()
        onBackgroundChange?(true)
    }
}
