import Foundation
import NotchCore

@MainActor
extension NotchModel {
    /// Typing stays enabled while a question is open; the question is shown inline in the chat (spec §4.7).
    var canCompose: Bool { session.phase != .listening }
    var hasRunningWork: Bool { activeTaskCount > 0 }
    var displayPhase: SessionPhase { !pendingApprovals.isEmpty ? .attention : (hasRunningWork && session.phase != .attention ? .working : session.phase) }
    var workStatus: String {
        let active = workSessions.values.filter { $0.activeTaskID != nil }
        if active.count > 1 { return "\(active.count) requests running" }
        return (session.activeTaskID != nil ? session : active.first)?.activity.last?.title ?? "Working…"
    }
    var visibleEntries: [TranscriptEntry] { conversation.transcript(on: followsToday ? Date() : selectedDay) }
    /// Open questions, shown at the end of today's chat.
    var inlineQuestions: [DaemonApproval] { viewingToday ? pendingApprovals : [] }
    /// The task a question belongs to: its goal, or its short id when the goal isn't known (a secret prompt must
    /// always say which task asks).
    func questionContext(_ approval: DaemonApproval) -> String? {
        guard let taskID = approval.taskID, !taskID.isEmpty else { return nil }
        if let goal = runningTasks[taskID], !goal.isEmpty { return goal }
        if let turn = conversation.turns.first(where: { $0.backendTaskID == taskID }) {
            let title = Self.title(of: turn)
            if title != Self.unknownTaskTitle { return title }
        }
        return "Task " + String(taskID.prefix(8))
    }
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
        guard !archiveBlocked else {
            promptError = "Earlier history couldn’t be read or set aside, so new messages aren’t being saved."
            return
        }
        do { preferences.set(try JSONEncoder().encode(conversation.forStorage()), forKey: "FERBConversationArchive") }
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

    func recordAnswer(announce: Bool, displayText: String? = nil, verified: Bool = false) {
        guard let receipt = session.result else { return }
        let existing = conversation.turns.first { turn in
            turn.id == receipt.id || (receipt.backendTaskID != nil && turn.backendTaskID == receipt.backendTaskID)
        }
        let id = existing?.id ?? receipt.id
        if receipt.isResumed {
            // A task this chat didn't send (reconnected or recovered): its request is the daemon's goal, never shown
            // as words the owner typed.
            conversation.begin(id: id, request: "", sources: receipt.sources, date: receipt.createdAt, goal: receipt.request)
        } else {
            conversation.begin(id: id, request: receipt.request, sources: receipt.sources, date: receipt.createdAt)
        }
        if let taskID = receipt.backendTaskID { conversation.bind(id: id, taskID: taskID) }
        conversation.finish(id: id, answer: receipt.answer, status: receipt.status, artifacts: receipt.artifacts,
            displayText: displayText, verified: verified)
        saveConversation()
        onBackgroundChange?(true)
    }
}
