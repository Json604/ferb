#if FERB_PLAYGROUND
import Foundation
import NotchCore

/// In-memory daemon records shared by the real window, chat and preview. Only sample artifacts use the fake home.
@MainActor
final class PlaygroundWorkspace {
    let state: PlaygroundState
    var enabled = false
    var scenario: PlaygroundChatScenario = .conversation
    var tasks = PlaygroundSamples.tasks
    var schedules = PlaygroundSamples.schedules
    var reminders = PlaygroundSamples.reminders
    var notices = PlaygroundSamples.notices
    var memories = PlaygroundSamples.memories
    var settings = PlaygroundSamples.settings
    var banners = Dictionary(uniqueKeysWithValues: BannerCategory.allCases.map { ($0.rawValue, true) })
    var approvals: [[String: Any]] = []
    /// Play 0.27.1 (PlaygroundPlay): the owner messages it sends, answered in order, and its questions.
    var playTurns: [PlayFeed] = []
    var playApprovals: Set<String> = []
    var stopped = false
    var listeners: [UUID: AsyncThrowingStream<[String: Any], Error>.Continuation] = [:]
    var seenNotices: Set<String> = []
    var seenResults: Set<String> = []
    var recovered = false
    var secrets: [[String: Any]] = [["place": "studio.example", "name": "Sample studio login", "kind": "password"]]
    var importFinished = false
    lazy var sampleImage = PlaygroundSamples.image()
    var artifact: URL { state.home.appendingPathComponent("Afternoon brief.txt") }

    var onNotification: (([String: Any]) -> Void)?

    init(state: PlaygroundState) { self.state = state }

    func prepare() throws {
        enabled = true
        seenResults.formUnion(tasks.compactMap { $0["task_id"] as? String })
        try FileManager.default.createDirectory(at: state.home.appendingPathComponent("logs"), withIntermediateDirectories: true)
        try "PLAYGROUND SAMPLE\n\nAfternoon brief\n\n1. Review the launch mockups.\n2. Choose a direction for the studio page.\n3. Leave 90 minutes for focused work.\n\nNo files, accounts or messages were accessed.\n".write(to: artifact, atomically: true, encoding: .utf8)
    }

    func emit(_ topic: String, _ data: [String: Any]) {
        let event: [String: Any] = ["topic": topic, "data": data, "at": PlaygroundSamples.time()]
        for listener in listeners.values { listener.yield(event) }
    }

    func answer(_ method: String, _ params: [String: Any]) throws -> [String: Any]? {
        if ["chat.approve", "chat.answer", "chat.correct"].contains(method),
           let id = params["approval_id"] as? String, playApprovals.contains(id) {
            // the play's question: answered here, and the play's script carries the task on
            playApprovals.remove(id)
            approvals.removeAll { $0["approval_id"] as? String == id }
            emit("approval.resolved", ["approval_id": id])
            return ["ok": true]
        }
        switch method {
        case "tasks.list": return ["tasks": tasks, "more": false]
        case "schedules.list": return ["schedules": schedules]
        case "reminders.list": return ["pending": reminders]
        case "schedules.change":
            let id = params["id"] as? String
            if let index = schedules.firstIndex(where: { $0["id"] as? String == id }) {
                let action = params["action"] as? String ?? ""
                if action == "cancel" { schedules.remove(at: index) }
                else { schedules[index]["state"] = action == "pause" ? "paused" : "active" }
            }
            return ["text": "Sample schedule updated."]
        case "reminders.cancel":
            reminders.removeAll { $0["id"] as? String == params["id"] as? String }
            return ["ok": true]
        case "notices.list": return ["notices": notices]
        case "attention.ack": seenNotices.formUnion(params["ids"] as? [String] ?? []); return ["ok": true]
        case "results.ack": seenResults.formUnion(params["task_ids"] as? [String] ?? []); return ["ok": true]
        case "memory.list": return ["items": memories]
        case "memory.profile":
            if let op = params["op"] as? String, let index = memories.firstIndex(where: { $0["id"] as? String == params["id"] as? String }) {
                memories[index]["in_profile"] = op == "add"
            }
            return ["sections": [["title": "Preferences", "items": memories.filter { $0["in_profile"] as? Bool == true }]],
                    "used_tokens": 34, "budget_tokens": 2000]
        case "memory.retract", "memory.forget":
            if method == "memory.forget" && params["confirm"] as? Bool != true { return ["will_delete": ["This sample memory"]] }
            memories.removeAll { $0["id"] as? String == params["id"] as? String }
            return ["ok": true]
        case "memory.edit":
            if let index = memories.firstIndex(where: { $0["id"] as? String == params["id"] as? String }) {
                memories[index]["content"] = params["text"] ?? params["content"]
            }
            return ["ok": true]
        case "memory.learning": return [:]
        case "memory.import.estimate": return ["name": "Sample assistant export", "conversations": 12, "model_calls_estimate": 3]
        case "memory.import.start": importFinished = true; return ["ok": true]
        case "memory.import.cancel": importFinished = false; return ["ok": true]
        case "memory.import.status":
            return importFinished ? ["current": ["run": "sample-import", "name": "Sample assistant export", "status": "done",
                                                "counts": ["chunks_done": 12, "facts_saved": 3, "model_calls": 3]]] : [:]
        case "settings.get": return ["values": settings]
        case "settings.set":
            if let key = params["key"] as? String { settings[key] = params["value"] }
            return ["ok": true, "value": params["value"] ?? NSNull()]
        case "notifications.categories":
            if let changed = params["set"] as? [String: Bool] { banners.merge(changed) { _, new in new } }
            return ["categories": banners]
        case "sense.status": return ["paused": state.saved.paused, "watchers": ["Sample apps", "Sample browser"]]
        case "stop.engage", "stop.lift":
            stopped = method == "stop.engage"
            if stopped { finishRunning(status: "cancelled", answer: "Stopped in the playground.") }
            return ["ok": true]
        case "task.stop":
            finishRunning(status: "cancelled", answer: "Stopped in the playground.", ids: params["task_ids"] as? [String])
            return ["ok": true]
        case "verdict.set":
            if let index = tasks.firstIndex(where: { $0["task_id"] as? String == params["task_id"] as? String }) { tasks[index]["verdict"] = params["verdict"] }
            return ["ok": true]
        case "chat.approve", "chat.answer", "chat.correct":
            let id = params["approval_id"] as? String
            let approval = approvals.first { $0["approval_id"] as? String == id }
            let task = approval?["task_id"] as? String
            let declined = params["granted"] as? Bool == false
                || (approval?["capability"] as? String == "ferb.confirm" && params["choice"] as? Int == 2)
            approvals.removeAll { $0["approval_id"] as? String == id }
            emit("approval.resolved", ["approval_id": id ?? ""])
            if let task { finishRunning(status: declined ? "cancelled" : "done", answer: declined
                ? "Cancelled. Nothing was sent or changed." : "Got it. The sample task is complete; nothing was sent or changed.", ids: [task]) }
            return ["ok": true]
        case "trace.get":
            let row = tasks.first { $0["task_id"] as? String == params["task_id"] as? String } ?? [:]
            return ["task": row, "events": [["kind": "sample", "text": "A simulated task for UI review; no tools ran."]]]
        case "tasks.preview": return ["state": "working", "fresh": false, "image": sampleImage, "line": "Reviewing the sample brief", "worker": "sample-worker"]
        case "seen.list": return ["days": 7, "total": 1, "captures": [["id": "sample-capture", "at": PlaygroundSamples.time(-900),
            "kind": "screen", "task_id": "sample-brief", "app": "Sample workspace", "title": "Afternoon brief", "tool": "Sample preview"]]]
        case "seen.image": return ["image": sampleImage]
        case "recovery.list": return ["days": 7, "total": recovered ? 0 : 1, "records": recovered ? [] : [["id": "sample-recovery",
            "kind": "copy", "original": artifact.path, "reason": "Sample edit", "at": PlaygroundSamples.time(-600), "saved_to": artifact.path]]]
        case "recovery.restore": recovered = true; return ["original": artifact.path, "restored_to": artifact.path, "in_place": true]
        case "secrets.saved": return ["entries": secrets]
        case "secrets.delete": secrets.removeAll { $0["name"] as? String == params["name"] as? String }; return ["ok": true]
        case "doctor.check": return ["selftest": [["name": "Playground services", "ok": true], ["name": "Sample data", "ok": true]]]
        case "reports.diagnostic", "reports.build": return try report()
        case "reports.send": return ["sent": false, "error": "Preview only. No diagnostic is sent from the playground."]
        case "about.get": return about()
        case "about.guide", "ui.click_through", "browser.show_handoff": return ["ok": true]
        case "update.prepare": return ["ready": true, "running": []]
        case "chat.status" where enabled:
            return ["paused": state.saved.paused, "stopped": stopped, "running": running,
                    "results": tasks.filter { ["done", "failed", "cancelled"].contains($0["status"] as? String ?? "") && !seenResults.contains($0["task_id"] as? String ?? "") },
                    "notices": notices.filter { $0["shown"] as? Bool != true && !seenNotices.contains($0["id"] as? String ?? "") }, "approvals": approvals]
        case "ping" where enabled: return ["pid": 0, "role": "app", "stopped": stopped]
        default: return nil
        }
    }

    var running: [[String: Any]] { tasks.filter { $0["status"] as? String == "running" } }

    /// The play's task, listed as running (chat.status, Tasks) until its outcome.
    func startPlay(task: String, request: String, goal: String) {
        let row: [String: Any] = ["task_id": task, "request_id": request, "goal": goal, "name": "Inbox, keyboard and practice",
                                  "status": "running", "created_at": PlaygroundSamples.time(),
                                  "started_at": PlaygroundSamples.time()]
        tasks.insert(row, at: 0)
        emit("task.created", row)
    }

    /// The play's task ended (or the play restarted): no longer running, its result already shown in its turn.
    func endPlay(task: String) {
        seenResults.insert(task)
        for index in tasks.indices where tasks[index]["task_id"] as? String == task {
            tasks[index]["status"] = "done"
        }
        let open = approvals.filter { playApprovals.contains($0["approval_id"] as? String ?? "") }
        approvals.removeAll { playApprovals.contains($0["approval_id"] as? String ?? "") }
        for approval in open { emit("approval.resolved", ["approval_id": approval["approval_id"] ?? ""]) }
        playApprovals.removeAll()
    }

    /// One of the play's owner messages: the ack, then its reply at once, or the task's events as the play pushes them.
    func playChat(_ feed: PlayFeed, yield: ([String: Any]) -> Void) async throws {
        yield(["type": "ack", "request_id": feed.requestID, "text": "On it.", "at": PlaygroundSamples.time()])
        if let events = feed.events {
            await state.pause(0.6)
            for event in events {
                try Task.checkCancellation()
                yield(event.merging(["at": PlaygroundSamples.time()]) { old, _ in old })
            }
            return
        }
        for await event in feed.stream {
            try Task.checkCancellation()
            yield(event.merging(["at": PlaygroundSamples.time()]) { old, _ in old })
        }
    }

    func finishRunning(status: String = "done", answer: String = "The sample brief is ready to review.", ids: [String]? = nil) {
        for index in tasks.indices where tasks[index]["status"] as? String == "running" {
            guard let id = tasks[index]["task_id"] as? String, ids == nil || ids!.contains(id) else { continue }
            let resolved = approvals.filter { $0["task_id"] as? String == id }
            approvals.removeAll { $0["task_id"] as? String == id }
            for approval in resolved { emit("approval.resolved", ["approval_id": approval["approval_id"] ?? ""]) }
            tasks[index]["status"] = status
            tasks[index]["answer"] = answer
            if var card = tasks[index]["card"] as? [String: Any], var rows = card["rows"] as? [[String: Any]] {
                for row in rows.indices { rows[row]["state"] = status == "done" ? "done" : "failed"; rows[row]["text"] = answer }
                card["rows"] = rows
                card["done"] = status == "done" ? rows.count : 0
                card["line"] = status == "done" ? "Finished" : status == "cancelled" ? "Stopped" : "Couldn't finish"
                tasks[index]["card"] = card
            }
            tasks[index]["artifacts"] = status == "done" ? [artifact.path] : []
            emit("task.result", tasks[index])
        }
    }

    private func report() throws -> [String: Any] {
        let file = state.home.appendingPathComponent("Sample diagnostic.txt")
        try "Playground sample diagnostic. No real account or machine data is included.\n".write(to: file, atomically: true, encoding: .utf8)
        return ["id": "sample-report", "subject": "Sample diagnostic", "path": file.path, "file": file.path, "summary": "Playground sample", "sent": false]
    }

    private func about() -> [String: Any] {
        ["version": ["ferb": "Playground"], "models": ["planner": "Sample AI", "worker": "Sample AI"],
         "account": ["email": "ferb@studio.example", "browser": "Sample browser", "profile": "FERB", "paired": true],
         "permissions": PlaygroundState.permissions.map { ["id": $0, "status": "granted"] },
         "records": [["records": "Sample records", "kept": "This preview session", "where": state.home.path]],
         "health": ["database_bytes": 2048000, "cpu_share_last_day": 0.01, "memory_now_bytes": 64000000],
         "cost": [:]]
    }
}
#endif
