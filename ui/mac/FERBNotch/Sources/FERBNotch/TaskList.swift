import AppKit
import Foundation
import SwiftUI
import NotchCore

struct TaskItem: Identifiable {
    let id: String
    let backendID: String?
    let localID: UUID?
    let title: String
    let status: String
    let activity: [TaskActivity]
    var answer: String? = nil
    var artifacts: [URL] = []
    var isRunning: Bool { ["working", "needs_user"].contains(status) }
}

@MainActor
extension NotchModel {
    var taskItems: [TaskItem] {
        var rows: [String: TaskItem] = [:]
        var sessions = workSessions
        if let id = session.activeTaskID { sessions[id] = session }
        for turn in conversation.turns where turn.backendTaskID?.isEmpty == false {
            let id = turn.backendTaskID!
            let active = runningTasks[id] != nil || sessions.values.contains { $0.backendTaskID == id && $0.activeTaskID != nil }
            let status = ["working", "needs_user"].contains(turn.status) && !active ? "Status unavailable" : turn.status
            rows[id] = TaskItem(id: id, backendID: id, localID: turn.id, title: turn.request,
                                status: status, activity: [], answer: turn.visibleAnswer, artifacts: turn.artifacts)
        }
        for (local, work) in sessions where work.activeTaskID != nil {
            let id = work.backendTaskID ?? local.uuidString
            let title = work.backendTaskID.flatMap { runningTasks[$0] }
                ?? conversation.turns.first(where: { $0.id == local })?.request
                ?? work.result?.request ?? work.draft
            rows[id] = TaskItem(id: id, backendID: work.backendTaskID, localID: local,
                title: title.isEmpty ? "New request" : title,
                status: work.phase == .attention ? "needs_user" : "working", activity: work.activity)
        }
        for (id, title) in runningTasks where !id.isEmpty {
            let old = rows[id]
            rows[id] = TaskItem(id: id, backendID: id, localID: old?.localID, title: title,
                status: pendingApprovals.contains(where: { $0.taskID == id }) ? "needs_user" : "working",
                activity: old?.activity ?? [])
        }
        return rows.values.sorted {
            if $0.isRunning != $1.isRunning { return $0.isRunning }
            return $0.id < $1.id
        }
    }

    var activeTaskCount: Int { taskItems.filter(\.isRunning).count }

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
        guard item.isRunning, !isStopping(item) else { return }
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
                Text("\(model.activeTaskCount) active").font(.system(size: 11)).foregroundStyle(Palette.secondary)
                Spacer()
                Button("Back", action: model.toggleTasks).buttonStyle(.plain)
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
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
                                if item.isRunning {
                                    Button(model.isStopping(item) ? "Stopping…" : "Stop task") { model.stopTask(item) }
                                        .buttonStyle(.plain).disabled(model.isStopping(item))
                                        .help("Stop only this task")
                                }
                            }.font(.system(size: 11))
                            if let approval = model.pendingApprovals.first(where: { item.backendID != nil && $0.taskID == item.backendID }) {
                                Button("Answer request") {
                                    model.selectedApprovalID = approval.id
                                    model.toggleTasks()
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
        if item.status == "working" { return item.activity.last?.title ?? "Working" }
        return item.status.replacingOccurrences(of: "_", with: " ").capitalized
    }
}
