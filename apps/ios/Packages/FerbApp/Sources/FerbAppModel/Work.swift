import FerbDesign
import FerbMobile
import Foundation

// The Tasks tab and the home screen's list (MobileScreens.swift PhoneTasksPage, HomePage), from the same transcript
// the chat draws, or from the Mac's own list (Android: model/Work.kt): what waits for the owner, what runs, what
// finished (PHN-13: "Tasks is a tab: what runs, how far, Stop per task, what ran before").

/// How far a task is: parts done of all.
public struct WorkProgress: Equatable, Sendable {
    public let done: Int
    public let total: Int

    public init(done: Int, total: Int) {
        self.done = done
        self.total = total
    }
}

/// One task as the Tasks tab and the home screen list it. `entry` opens it in the chat; `approvalId` its question.
public struct WorkItem: Equatable, Sendable {
    public var kind: MarkKind
    public var title: String
    public var detail: String
    public var progress: WorkProgress? = nil
    public var entry: String? = nil
    public var approvalId: String? = nil
    public var taskId: String? = nil

    public init(kind: MarkKind, title: String, detail: String, progress: WorkProgress? = nil, entry: String? = nil,
                approvalId: String? = nil, taskId: String? = nil) {
        self.kind = kind
        self.title = title
        self.detail = detail
        self.progress = progress
        self.entry = entry
        self.approvalId = approvalId
        self.taskId = taskId
    }
}

public struct Work: Equatable, Sendable {
    public var waiting: [WorkItem]
    public var running: [WorkItem]
    public var earlier: [WorkItem]

    public init(waiting: [WorkItem], running: [WorkItem], earlier: [WorkItem]) {
        self.waiting = waiting
        self.running = running
        self.earlier = earlier
    }

    public static let empty = Work(waiting: [], running: [], earlier: [])
    public var isEmpty: Bool { waiting.isEmpty && running.isEmpty && earlier.isEmpty }
}

public enum WorkModel {
    /// `asking` is the app's line for a question with no title of its own.
    public static func of(_ view: TranscriptView?, asking: String) -> Work {
        guard let view else { return .empty }
        var live: [String: LiveView] = [:]
        for l in view.live { live[l.entry] = l }
        let turns: [TurnItem] = view.items.compactMap { if case .turn(let t) = $0 { return t } else { return nil } }
        var byTask: [String: TurnItem] = [:]
        for t in turns { if let id = t.taskId { byTask[id] = t } }
        var waiting = Questions.open(view.questions).map { q -> WorkItem in
            let turn = q.taskId.flatMap { byTask[$0] }
            return WorkItem(kind: .asking, title: turn.map(title) ?? (q.title ?? asking), detail: q.title ?? oneLine(q.reason),
                            entry: turn?.entry, approvalId: q.approvalId, taskId: q.taskId)
        }
        var running: [WorkItem] = []
        var earlier: [WorkItem] = []
        for t in turns.reversed() {
            if t.taskId == nil && t.card == nil && t.checklist == nil { continue }
            let l = live[t.entry]
            let prog = progress(t)
            if l != nil || t.status == "working" || t.status == "needs_user" {
                let detail = l?.line?.text ?? l?.activity ?? l?.rows.first(where: \.spinner)?.words ?? t.firstReply?.planLine ?? ""
                let kind: MarkKind = (l?.state == "needs_you" || t.status == "needs_user") ? .asking : .running
                place(WorkItem(kind: kind, title: title(t), detail: detail, progress: prog, entry: t.entry, taskId: t.taskId),
                      &waiting, &running)
            } else if t.status != "merged" {
                let said = t.answer.map { oneLine(ChatModel.plain($0.displayText ?? $0.text)) } ?? ""
                earlier.append(WorkItem(kind: ChatModel.markOf(t.status), title: title(t), detail: said, entry: t.entry, taskId: t.taskId))
            }
        }
        return Work(waiting: waiting, running: running, earlier: earlier)
    }

    /// The Tasks tab from the Mac's own list (`tasks.list`: every task, not only the ones this chat started): what
    /// waits for the owner is `waiting` (the open questions); a task runs while its status is not a finished one;
    /// `chat` gives the chat entry of a task this conversation holds, so a tap opens it there.
    public static func fromMac(_ rows: [TaskRow], waiting: [WorkItem], chat: Work) -> Work {
        var entries: [String: String?] = [:]
        for w in chat.running + chat.earlier { if let id = w.taskId { entries[id] = w.entry } }
        var waiting = waiting
        var running: [WorkItem] = []
        var earlier: [WorkItem] = []
        for r in rows {
            let title = oneLine(r.name.isEmpty ? r.goal : r.name)
            let progress = r.stepsTotal.flatMap { $0 > 0 ? WorkProgress(done: Int(r.stepsDone ?? 0), total: Int($0)) : nil }
            let mark = ChatModel.markOf(r.status)
            let finished = finishedStates.contains(r.status) || mark == .done || mark == .failed
            let entry = entries[r.taskId] ?? nil
            if !finished {
                let kind: MarkKind = mark == .asking ? .asking : .running
                place(WorkItem(kind: kind, title: title, detail: oneLine(r.goal != title ? r.goal : ""), progress: progress,
                               entry: entry, taskId: r.taskId), &waiting, &running)
            } else {
                earlier.append(WorkItem(kind: mark, title: title, detail: oneLine(ChatModel.plain(r.answer)), entry: entry, taskId: r.taskId))
            }
        }
        return Work(waiting: waiting, running: running, earlier: earlier)
    }

    /// A task that waits for the owner is listed under what waits for him, never as running; once only, so a task
    /// whose open question is listed there already isn't listed again (owner 2026-11-23: two ChatGPT watches asking
    /// him were under Running).
    private static func place(_ item: WorkItem, _ waiting: inout [WorkItem], _ running: inout [WorkItem]) {
        if item.kind != .asking { running.append(item) }
        else if item.taskId == nil || !waiting.contains(where: { $0.taskId == item.taskId }) { waiting.append(item) }
    }

    private static let finishedStates: Set<String> = [
        "done", "failed", "stopped", "cancelled", "interrupted", "done_with_gaps", "done_unverified", "merged",
    ]

    private static func progress(_ t: TurnItem) -> WorkProgress? {
        if let c = t.card { return WorkProgress(done: Int(c.done), total: Int(c.total)) }
        if let c = t.checklist { return WorkProgress(done: c.ended.count, total: c.steps.count) }
        return nil
    }

    public static func title(_ t: TurnItem) -> String { oneLine(t.taskName ?? t.words?.text ?? t.goal ?? "") }

    public static func oneLine(_ s: String) -> String {
        let line = (s.lines.first { !$0.isBlank } ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        return line.count > 90 ? String(line.prefix(89)) + "…" : line
    }
}
