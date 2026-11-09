import Foundation
import NotchCore

@MainActor
extension NotchModel {
    /// Typing stays enabled while a question is open; the question is shown inline in the chat (spec §4.7).
    var canCompose: Bool { session.phase != .listening }
    var hasRunningWork: Bool { activeTaskCount > 0 }
    var displayPhase: SessionPhase {
        if !pendingApprovals.isEmpty { return .attention }
        if hasRunningWork && session.phase != .attention { return .working }
        return session.phase == .working && isStanding(session) ? .ready : session.phase
    }
    /// A newly created background task takes precedence over an older approval/result color on the compact marker.
    var indicatorSignal: String {
        "\(taskStateRevision):\(hasRunningWork ? SessionPhase.working : displayPhase):\(runningTasks.keys.sorted())"
    }
    var indicatorPhase: SessionPhase {
        if expanded || acknowledgedIndicatorSignal == indicatorSignal { return .ready }
        return hasRunningWork ? .working : displayPhase
    }
    var workStatus: String {
        let active = workSessions.values.filter { $0.activeTaskID != nil && !isStanding($0) }
        if active.count > 1 { return "\(active.count) requests running" }
        return (session.activeTaskID != nil && !isStanding(session) ? session : active.first)?.activity.last?.title ?? "Working…"
    }
    /// A session whose task is a loop between its rounds (L31): standing work, not running work, until a round acts.
    func isStanding(_ work: NotchSession) -> Bool {
        guard let id = work.backendTaskID, let loop = standingWork[id] else { return false }
        return loop.state != "acting"
    }
    var visibleEntries: [TranscriptEntry] {
        let day = Calendar.current.startOfDay(for: followsToday ? Date() : selectedDay)
        if let shown = shownTranscript, shown.day == day { return shown.entries }
        let entries = conversation.transcript(on: day)
        shownTranscript = (day, entries)
        return entries
    }
    /// Open questions, shown at the end of today's chat; one that belongs to a card row shown here is in that row instead
    /// (APP-300).
    var inlineQuestions: [DaemonApproval] {
        guard viewingToday else { return [] }
        let placed = conversation.rowPlacements(pendingApprovals)
        guard !placed.isEmpty else { return pendingApprovals }
        let shown = Set(visibleEntries.compactMap { entry -> UUID? in
            if case let .turn(turn, _) = entry { return turn.id } else { return nil }
        })
        return pendingApprovals.filter { approval in placed[approval.id].map { !shown.contains($0.turnID) } ?? true }
    }
    /// The open questions shown inside this turn's card, by part (APP-300). Live state: only in today's chat.
    func rowQuestions(_ turn: ConversationTurn) -> [String: [DaemonApproval]] {
        guard viewingToday, turn.card != nil, !pendingApprovals.isEmpty else { return [:] }
        let placed = conversation.rowPlacements(pendingApprovals)
        return Dictionary(grouping: pendingApprovals.filter { placed[$0.id]?.turnID == turn.id }) { placed[$0.id]!.part }
    }
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

    /// `focus: false`: FERB itself shows the day (the intro), without taking focus (rule 10).
    func selectDay(_ day: Date, focus: Bool = true) {
        followsToday = Calendar.current.isDateInToday(day)
        measuredTranscriptHeight = 0
        selectedDay = Calendar.current.startOfDay(for: day)
        showingHistory = false
        showingSources = false
        showingActivity = false
        if focus {
            onInteraction?()
            onPresentationChange?(true)
        } else {
            onBackgroundChange?(true)
        }
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
            conversation.begin(id: id, request: receipt.request, sources: receipt.sources, date: receipt.createdAt, unredacted: true)
        }
        if let taskID = receipt.backendTaskID { conversation.bind(id: id, taskID: taskID) }
        conversation.finish(id: id, answer: receipt.answer, status: receipt.status, artifacts: receipt.artifacts,
            displayText: displayText, verified: verified)
        saveConversation()
        onBackgroundChange?(true)
    }
}
