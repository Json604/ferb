import FerbMobile
import Foundation

/// The temporary chat as the phone draws it while it is on (the owner, 2026-11-23: one chat the Mac holds in memory
/// only, with every tool and no prior context): its rows from the library's fold, as the chat's own rows are made, and
/// the tasks its turns started (their questions are its). Memory only: nothing of it is saved on the phone.
public struct TemporaryShown: Equatable {
    public let id: String?
    public let rev: Int64
    public let startedAt: String?
    public let rows: [ChatRow]
    /// What each of its bubbles offers as a quote and what his messages quote (kb APP-1146), as the chat's `replies`.
    public let replies: [String: RowReply]
    public let taskIds: Set<String>
    /// Its turns as the library folded them: the work row of its tasks is made from them.
    public let transcript: TranscriptView?

    public init(id: String?, rev: Int64, startedAt: String?, rows: [ChatRow], taskIds: Set<String>, transcript: TranscriptView? = nil) {
        self.transcript = transcript
        self.id = id
        self.rev = rev
        self.startedAt = startedAt
        self.rows = rows
        self.replies = ChatModel.replies(transcript)
        self.taskIds = taskIds
    }

    /// From the library's answer (FERB's words already laid out off the main actor, LibraryPhone.laidOut). `sending`:
    /// his message on its way, drawn as the chat draws his outbox (at the end, "Sending…"; APP-1126).
    public init(_ t: TemporaryChat, sending: OutboxItem? = nil) {
        var tasks = Set<String>()
        for item in t.transcript.items { if case .turn(let turn) = item, let id = turn.taskId { tasks.insert(id) } }
        var shown = t.transcript
        if let sending { shown.outbox = [sending] }
        self.init(id: t.id, rev: t.rev, startedAt: t.startedAt, rows: ChatModel.rows(shown), taskIds: tasks, transcript: t.transcript)
    }

    /// What the chat tab draws while it is on: its rows, then the open questions its tasks ask that its rows don't
    /// hold yet (the Mac's status brought them first), as the chat's loose questions (kb PHN-770).
    public func shown(_ questions: [Question]) -> [ChatRow] {
        let placed = Set(rows.compactMap { if case .asking(_, let id, _, _, _) = $0 { return id } else { return nil } })
        let loose: [ChatRow] = questions.filter { q in !placed.contains(q.approvalId) && owns(q) }
            .map { .asking(key: "q:\($0.approvalId)", approvalId: $0.approvalId, title: $0.title, reason: nil, hidden: $0.hidden) }
        return rows + loose
    }

    /// The questions that are the temporary chat's (asked by its tasks, or held by its rows): never loose in the chat.
    public func owns(_ q: Question) -> Bool {
        if q.temporary || q.taskId.map(taskIds.contains) == true { return true }
        return rows.contains { if case .asking(_, let id, _, _, _) = $0 { return id == q.approvalId } else { return false } }
    }
}

/// The Mac's status with only the temporary chat's rows (`temporary`) or only the others: its questions and running
/// tasks are shown only in it, and the chat's never in it (the status rows' `temporary` flag).
func statusPart(_ status: PhoneStatus?, temporary: Bool) -> PhoneStatus? {
    guard var s = status else { return nil }
    s.questions = s.questions.filter { $0.temporary == temporary }
    s.running = s.running.filter { $0.temporary == temporary }
    return s
}
