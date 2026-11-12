import AppKit
import Foundation
import SwiftUI
import NotchCore

struct TaskItem: Identifiable, Equatable {
    let id: String
    let backendID: String?
    let localID: UUID?
    let title: String
    let status: String
    let activity: [TaskActivity]
    var answer: String? = nil
    var artifacts: [URL] = []
    /// A running loop's line (L31); its task is standing work while no round is acting.
    var standing: String? = nil
    var isRunning: Bool { Self.isRunning(status) }
    /// Paused in place by the owner (ORCH-308, the daemon's `task.paused`): not running and not ended; it resumes in
    /// place.
    var isPaused: Bool { status == "paused" }
    /// Running or paused in place: its round picture stays by the work row (frozen while paused, APP-404).
    var isUnderway: Bool { isRunning || isPaused }
    /// Still going: running, paused, or a loop kept between its rounds. Any of them can be stopped.
    var isOngoing: Bool { isUnderway || standing != nil }
    /// APP-890: what FERB is doing now, the one line its short view and its own box show (the whole list stays in
    /// Show activity and the Tasks page). APP-893: the daemon's plain live line (`task.progress`), worded as a card
    /// row's, never a tool's name.
    var currentLine: String { activity.last.map { RowDisplay.sentence($0.title) } ?? "Starting task…" }
    static func isRunning(_ status: String) -> Bool { status == "working" || status == "needs_user" }
}

/// The chat's work row in words (APP-901, owner 2026-11-12: "this paused task shoudl also relfect in these line, loader
/// adn time"). The timer sits between `lead` and `trail` and counts only running tasks: "1 active task · 2:10 · 1
/// paused". With nothing running there is no timer: "1 paused task", "2 paused tasks · 1 kept going".
struct WorkRowWords: Equatable {
    let lead: String
    let trail: String

    init(active: Int, paused: Int, kept: Int) {
        func tasks(_ n: Int) -> String { n == 1 ? "task" : "tasks" }
        var rest: [String] = []
        if paused > 0 { rest.append(active > 0 ? "\(paused) paused" : "\(paused) paused \(tasks(paused))") }
        if kept > 0 { rest.append("\(kept) kept going") }
        if active > 0 {
            lead = "\(active) active \(tasks(active))"
            trail = rest.joined(separator: " · ")
        } else {
            lead = rest.joined(separator: " · ")
            trail = ""
        }
    }

    /// The row's words without the timer.
    var text: String { [lead, trail].filter { !$0.isEmpty }.joined(separator: " · ") }
}

/// The task list as last built. The notch's size reads it on every pointer move, animation frame and keystroke, so it
/// is built once per change of what it reads, never per read (owner, 2026-10-25: the notch lagged as history grew).
/// Every input invalidates it where it is set (NotchModel.swift); a loop's line reads the clock, so each read makes it.
struct TaskListCache {
    struct Built {
        let rows: [TaskItem]
        /// The rows of loops kept going, whose line is made again at each read.
        let standing: [Int]
        let active: Int
        let paused: Int
        let kept: Int
    }

    fileprivate var built: Built?
    /// How many times the list was built (tests count rebuilds).
    fileprivate(set) var builds = 0

    mutating func invalidate() { built = nil }
}

@MainActor
extension NotchModel {
    /// Running first, then kept loops, then the newest first; a task with no turn here (recovered from the daemon)
    /// counts as newest. Never the random task id's order (owner 2026-10-25: "it shows previous on top").
    var taskItems: [TaskItem] {
        let list = builtTaskList
        guard !list.standing.isEmpty else { return list.rows }
        var rows = list.rows
        let now = Date()
        for index in list.standing { rows[index].standing = standingLine(rows[index].id, now: now) }
        return rows
    }

    var activeTaskCount: Int { builtTaskList.active }

    /// Tasks the owner paused (ORCH-308): counted apart from the active ones, with no spinner and no running time.
    var pausedTaskCount: Int { builtTaskList.paused }

    /// Loops kept between their rounds (L31): standing work, counted apart from the active tasks.
    var keptGoingCount: Int { builtTaskList.kept }

    /// The chat's work row: shown while a task runs or is paused, or a loop is kept; its spinner turns only for
    /// running work.
    var showsWorkRow: Bool { hasRunningWork || pausedTaskCount > 0 || keptGoingCount > 0 }

    /// When the longest-running active task started: the work row's timer counts from it ("1 active task · 0:42"). A
    /// paused task's time isn't counted (APP-901): with every task paused the row shows no time.
    var workSince: Date? {
        guard hasRunningWork else { return nil }
        return runningSince.filter { !pausedTasks.contains($0.key) }.values.min()
    }

    /// The work row's words on either side of its timer (APP-901).
    var workRowWords: WorkRowWords {
        WorkRowWords(active: activeTaskCount, paused: pausedTaskCount, kept: keptGoingCount)
    }

    var workRowText: String { workRowWords.text }

    private var builtTaskList: TaskListCache.Built {
        if let built = taskListCache.built { return built }
        let built = makeTaskList()
        taskListCache.built = built
        taskListCache.builds += 1
        return built
    }

    /// The window's session changed. The list reads it only while it holds a task, and its draft only when that draft
    /// is the task's title, so typing, opening and closing keep the list.
    func taskListSessionChanged(from old: NotchSession) {
        guard taskListCache.built != nil else { return }
        if Self.taskFields(old) != Self.taskFields(session) || old.draft != session.draft && draftIsTaskTitle(session) {
            taskListCache.invalidate()
        }
    }

    private struct SessionTaskFields: Equatable {
        let local: UUID
        let backendTaskID: String?
        let attention: Bool
        let request: String?
        let activity: [TaskActivity]
    }

    /// What the list reads from a session that holds a task (`makeTaskList`), apart from its draft.
    private static func taskFields(_ work: NotchSession) -> SessionTaskFields? {
        guard let local = work.activeTaskID else { return nil }
        return SessionTaskFields(local: local, backendTaskID: work.backendTaskID, attention: work.phase == .attention,
                                 request: work.result?.request, activity: work.activity)
    }

    /// The draft is the row's title only when nothing before it names the task (`makeTaskList`).
    private func draftIsTaskTitle(_ work: NotchSession) -> Bool {
        guard let local = work.activeTaskID, work.result?.request == nil,
              work.backendTaskID.flatMap({ runningTasks[$0] }) == nil else { return false }
        return !conversation.turns.contains { $0.id == local }
    }

    /// Builds the list from the conversation, the task sessions, the daemon's running tasks, open questions and loops.
    func makeTaskList() -> TaskListCache.Built {
        var rows: [String: TaskItem] = [:]
        var asked: [String: Int] = [:]          // the conversation's order: a later turn was asked later
        var sessions = workSessions
        if let id = session.activeTaskID { sessions[id] = session }
        for (index, turn) in conversation.turns.enumerated() where turn.backendTaskID?.isEmpty == false {
            let id = turn.backendTaskID!
            asked[id] = index
            let active = runningTasks[id] != nil || sessions.values.contains { $0.backendTaskID == id && $0.activeTaskID != nil }
            let status = TaskItem.isRunning(turn.status) && !active ? "Status unavailable" : turn.status
            rows[id] = TaskItem(id: id, backendID: id, localID: turn.id, title: Self.title(of: turn),
                                status: status, activity: [], answer: turn.visibleAnswer, artifacts: turn.artifacts)
        }
        for (local, work) in sessions where work.activeTaskID != nil {
            let id = work.backendTaskID ?? local.uuidString
            let title = work.backendTaskID.flatMap { runningTasks[$0] }
                ?? conversation.turns.first(where: { $0.id == local }).map(Self.title(of:))
                ?? work.result?.request ?? work.draft
            rows[id] = TaskItem(id: id, backendID: work.backendTaskID, localID: local,
                title: title.isEmpty ? (work.backendTaskID == nil ? "New request" : "A task from earlier") : title,
                status: work.phase == .attention ? "needs_user" : "working",
                activity: work.backendTaskID.flatMap { taskActivities[$0] } ?? work.activity)
        }
        let now = Date()
        for (id, title) in runningTasks where !id.isEmpty {
            let old = rows[id]
            // a question of its own shows first, then the owner's pause (ORCH-308, APP-901)
            let status = pendingApprovals.contains(where: { $0.taskID == id }) ? "needs_user"
                : pausedTasks.contains(id) ? "paused"
                : standingWork[id].map { $0.state == "acting" ? "working" : "standing" } ?? "working"
            rows[id] = TaskItem(id: id, backendID: id, localID: old?.localID, title: title, status: status,
                activity: taskActivities[id] ?? old?.activity ?? [], standing: standingLine(id, now: now))
        }
        // Each row's place is worked out once, not in every comparison.
        let sorted = rows.values.map { ($0, $0.isRunning, $0.isOngoing, asked[$0.id] ?? Int.max) }.sorted {
            if $0.1 != $1.1 { return $0.1 }
            if $0.2 != $1.2 { return $0.2 }
            return $0.3 != $1.3 ? $0.3 > $1.3 : $0.0.id < $1.0.id
        }.map(\.0)
        return TaskListCache.Built(rows: sorted, standing: sorted.indices.filter { sorted[$0].standing != nil },
                                   active: sorted.filter(\.isRunning).count,
                                   paused: sorted.filter(\.isPaused).count,
                                   kept: sorted.filter { $0.status == "standing" }.count)
    }

    /// "Keeping: <goal> · fixed N times · last X min ago", plus the loop's state when it waits longer or needs you.
    func standingLine(_ id: String, now: Date) -> String? {
        guard let work = standingWork[id] else { return nil }
        var parts = ["Keeping: \(work.goal)"]
        if work.rounds > 0 { parts.append("fixed \(work.rounds) \(work.rounds == 1 ? "time" : "times")") }
        if let end = work.lastRoundEnd {
            let minutes = max(0, Int(now.timeIntervalSince(end) / 60))
            parts.append(minutes == 0 ? "last just now" : minutes < 60 ? "last \(minutes) min ago" : "last \(minutes / 60) hr ago")
        }
        if work.state == "backing_off" { parts.append("waiting longer between fixes") }
        if work.state == "paused" { parts.append("paused: needs you") }
        return parts.joined(separator: " · ")
    }

    /// The task's short name (T-1055), else the owner's message, or for a task this chat didn't send, the daemon's
    /// goal (already its heading). Never an acknowledgement an older daemon sent (APP-812).
    static func title(of turn: ConversationTurn) -> String {
        if let name = turn.taskName, !name.isEmpty { return name }
        if !turn.request.isEmpty { return turn.request }
        if let goal = turn.taskGoal, !goal.isEmpty { return goal }
        return turn.plan ?? unknownTaskTitle
    }

    static let unknownTaskTitle = "A task from earlier"

    func toggleTasks() {
        showingTasks.toggle()
        if !showingTasks { clearTaskInspection() }
        onInteraction?()
        onPresentationChange?(true)
    }

    func clearTaskInspection() {
        taskInspectionJob?.cancel()
        taskInspectionJob = nil
        taskInspectionRequest = UUID()
        inspectedTaskID = nil
        taskInspection = nil
        taskRecordURL = nil
    }

    func inspectTask(_ item: TaskItem) {
        clearTaskInspection()
        inspectedTaskID = item.id
        let request = taskInspectionRequest
        guard let backend = item.backendID, !backend.isEmpty else {
            taskInspection = "Waiting for FERB to assign this request a task."
            return
        }
        taskInspectionJob = Task {
            do {
                let trace = try await rpc.call("trace.get", params: ["task_id": backend])
                guard taskInspectionRequest == request, !Task.isCancelled else { return }
                if let outcome = trace["outcome"] as? [String: Any],
                   let result = outcome["result"] as? [String: Any], let status = result["status"] as? String,
                   let turn = conversation.turns.first(where: { $0.backendTaskID == backend }) {
                    conversation.finish(id: turn.id, answer: result["answer"] as? String ?? result["summary"] as? String ?? turn.answer,
                        status: status, artifacts: (result["artifacts"] as? [String] ?? []).filter { $0.hasPrefix("/") }.map { URL(fileURLWithPath: $0) },
                        displayText: result["display_text"] as? String, verified: result["verified"] as? Bool ?? false)
                    saveConversation()
                }
                let url = try await taskRecordFiles.write(trace)
                guard taskInspectionRequest == request, !Task.isCancelled else {
                    taskRecordFiles.discard(url)
                    return
                }
                taskRecordURL = url
            } catch {
                guard taskInspectionRequest == request, !Task.isCancelled else { return }
                taskInspection = error.localizedDescription
            }
        }
    }

    func isStopping(_ item: TaskItem) -> Bool {
        stoppingTaskIDs.contains(item.id) || item.localID.map { pendingStops.contains($0) } == true
    }

    func stopTask(_ item: TaskItem) {
        guard item.isOngoing, !isStopping(item) else { return }
        guard let backend = item.backendID else {
            if let local = item.localID { pendingStops.insert(local) }
            return
        }
        guard !backend.isEmpty else { return }
        stoppingTaskIDs.insert(backend)
        Task {
            do {
                _ = try await rpc.call("task.stop", params: ["task_ids": [backend]])
                await refreshDaemonStatus()
            } catch {
                promptError = error.localizedDescription
            }
            stoppingTaskIDs.remove(backend)
        }
    }
}

struct TaskListView: View {
    @ObservedObject var model: NotchModel

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("Tasks").font(.system(size: 13, weight: .semibold))
                Text("\(model.activeTaskCount) active"
                     + (model.pausedTaskCount > 0 ? " · \(model.pausedTaskCount) paused" : "")
                     + (model.keptGoingCount > 0 ? " · \(model.keptGoingCount) kept going" : ""))
                    .font(.system(size: 11)).foregroundStyle(Palette.secondary)
                Spacer()
                Button("Back", action: model.toggleTasks).buttonStyle(.plain)
            }
            ScrollView {
                // Lazy: a day of a hundred tasks opens and updates as fast as a few (owner 2026-11-08).
                LazyVStack(alignment: .leading, spacing: 14) {
                    if model.taskItems.isEmpty { Text("No tasks yet.").foregroundStyle(Palette.secondary) }
                    ForEach(model.taskItems) { item in
                        VStack(alignment: .leading, spacing: 7) {
                            Text(item.title).font(.system(size: 12, weight: .medium)).textSelection(.enabled)
                                .lineLimit(model.inspectedTaskID == item.id ? nil : 2)
                            HStack {
                                Text(status(item)).font(.system(size: 10)).foregroundStyle(Palette.secondary)
                                Spacer()
                                Button(model.inspectedTaskID == item.id ? "Hide details" : "Inspect") {
                                    if model.inspectedTaskID == item.id { model.clearTaskInspection() }
                                    else { model.inspectTask(item) }
                                }.buttonStyle(.plain)
                                if item.isOngoing {
                                    Button(model.isStopping(item) ? "Stopping…" : "Stop task") { model.stopTask(item) }
                                        .buttonStyle(.plain).disabled(model.isStopping(item))
                                        .help("Stop only this task")
                                }
                            }.font(.system(size: 11))
                            if let approval = model.pendingApprovals.first(where: { item.backendID != nil && $0.taskID == item.backendID }) {
                                Button("Answer request") {
                                    // The question is inline at the end of today's chat; the owner asked to answer it.
                                    model.toggleTasks()
                                    model.focusQuestion(approval.id)
                                }.buttonStyle(FerbButtonStyle(quiet: true))
                            }
                            if model.inspectedTaskID == item.id {
                                if let answer = item.answer, !answer.isEmpty {
                                    MarkdownContent(answer).equatable().font(.system(size: 12))
                                }
                                ForEach(item.artifacts, id: \.self) { url in
                                    Button { model.onOpenDocument?(url) } label: {
                                        Label(url.lastPathComponent, systemImage: "doc")
                                    }.buttonStyle(FerbButtonStyle(quiet: true))
                                }
                                if let backend = item.backendID {
                                    Text("Task " + backend).font(.system(size: 9, design: .monospaced))
                                        .foregroundStyle(Palette.secondary).textSelection(.enabled)
                                }
                                ForEach(item.activity) { event in
                                    Label(event.title, systemImage: event.symbol).font(.system(size: 11)).textSelection(.enabled)
                                }
                                if let url = model.taskRecordURL {
                                    HStack {
                                        Button("Open full task record") { model.onOpenDocument?(url) }
                                        Spacer()
                                        Button("Reveal") { NSWorkspace.shared.activateFileViewerSelecting([url]) }
                                    }.buttonStyle(.plain).font(.system(size: 11))
                                } else {
                                    Text(model.taskInspection ?? "Preparing full task record…")
                                        .font(.system(size: 11)).foregroundStyle(Palette.secondary)
                                }
                            }
                        }.padding(10).frame(maxWidth: .infinity, alignment: .leading)
                            .background(.white.opacity(0.055), in: RoundedRectangle(cornerRadius: 10))
                    }
                }
            }
        }
    }

    private func status(_ item: TaskItem) -> String {
        if item.backendID == nil && item.isRunning { return "Sending request" }
        if item.status == "needs_user" { return "Needs your answer" }
        if let standing = item.standing { return standing }
        if item.status == "working" { return item.activity.last?.title ?? "Working" }
        return item.status.replacingOccurrences(of: "_", with: " ").capitalized
    }
}
