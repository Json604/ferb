#if FERB_PLAYGROUND
import Foundation
import NotchCore

/// Explicit review states. Typed messages use the selected state; no model or intent classifier runs.
enum PlaygroundChatScenario: String, CaseIterable, Identifiable {
    case conversation = "Chat reply", working = "Running task", approval = "Approval", question = "Question"
    case completed = "Finished result", reminder = "Reminder arriving", schedule = "Scheduled result"
    case failed = "Failed task", cards = "All message cards"
    var id: String { rawValue }
    var request: String {
        switch self {
        case .conversation: return "Help me make space for focused work this afternoon."
        case .working, .completed: return "Prepare an afternoon brief from the sample launch notes."
        case .approval: return "Send Alex the launch brief."
        case .question: return "Put the launch notes in the right project folder."
        case .reminder: return "Remind me to take a break in 20 minutes."
        case .schedule: return "Bring me a short morning brief every weekday."
        case .failed: return "Collect this week's studio invoices."
        case .cards: return "Show me the different kinds of things FERB can return."
        }
    }
}

@MainActor
extension PlaygroundWorkspace {
    func chat(_ request: String, yield: ([String: Any]) -> Void) async throws {
        if !playTurns.isEmpty { return try await playChat(playTurns.removeFirst(), yield: yield) }
        let selected = scenario
        let requestID = "sample-request-" + UUID().uuidString
        yield(["type": "ack", "request_id": requestID, "text": "On it.", "at": PlaygroundSamples.time()])
        await state.pause(0.35)
        try Task.checkCancellation()
        if [.working, .approval, .question, .completed, .failed].contains(selected) {
            let id = "sample-task-" + UUID().uuidString
            let row: [String: Any] = ["part": "1", "label": "Prepare the brief", "state": "working", "step_task_id": id + ".1"]
            let card: [String: Any] = ["request_id": requestID, "rows": [row], "done": 0, "total": 1]
            tasks.insert(["task_id": id, "request_id": requestID, "goal": request, "status": "running", "card": card,
                          "created_at": PlaygroundSamples.time(), "started_at": PlaygroundSamples.time()], at: 0)
            emit("task.created", tasks[0])
            yield(["type": "plan", "task_id": id, "request_id": requestID, "goal": request,
                   "text": "I'll prepare the sample brief, then check the result.", "card": card])
            if selected == .approval || selected == .question {
                let approval: [String: Any] = ["approval_id": "sample-approval-" + UUID().uuidString, "task_id": id,
                    "kind": "owner_question", "part": "1", "source_task_id": id + ".1",
                    "capability": selected == .approval ? "ferb.confirm" : "owner.ask",
                    "reason": selected == .approval ? "Send the launch brief to Alex at alex@studio.example from the sample studio account?" : "Which project should these notes go in?",
                    "summary": selected == .approval ? "Send the brief to Alex?" : "Which project folder?",
                    "why": "This is a playground example. No message or file will leave this preview.",
                    "options": selected == .question ? ["Studio launch", "Website refresh"] : ["Yes", "No"]]
                approvals.append(approval)
                emit("approval.requested", approval)
            } else if selected != .working {
                await state.pause(1.4)
                try Task.checkCancellation()
                finishRunning(status: selected == .failed ? "failed" : "done", answer: selected == .failed
                    ? "I couldn't connect to the sample account. Reconnect it, then try again."
                    : "Your afternoon brief is ready. Start with the mockups, make the two open decisions, then protect a 90-minute focus block.", ids: [id])
            }
            return
        }
        let text: String
        let cards: [[String: Any]]
        switch selected {
        case .reminder:
            let id = "sample-reminder-" + UUID().uuidString
            let at = PlaygroundSamples.time(1200)
            reminders.append(["id": id, "kind": "reminder", "at": at, "message": "Take a break and get some water",
                              "name": "Water break"])
            text = "I'll remind you in 20 minutes."
            cards = [["type": "event", "kind": "reminder", "id": id, "what": "Water break", "when": at]]
        case .schedule:
            text = "Your morning brief is ready: two updates need a decision, and your afternoon has room for focused work."
            cards = [["type": "reminder", "title": "Morning brief", "due": PlaygroundSamples.time(),
                      "repeat": "Weekdays at 9:00 AM", "schedule_id": "sample-morning", "next_run": PlaygroundSamples.next(hour: 9, weekdaysOnly: true)]]
        case .cards:
            text = "Sample results, all in one conversation. Nothing here was sent, purchased or changed."
            cards = sampleCards
        default:
            text = "You have room for one good stretch of focused work.\n\nStart with the launch mockups, settle the two open decisions, then keep **2:00–3:30 PM** clear. Leave messages and small tasks for afterwards.\n\nWant me to turn that into a reminder or a recurring plan?"
            cards = []
        }
        for word in text.split(separator: " ", omittingEmptySubsequences: false) {
            try Task.checkCancellation()
            yield(["type": "reply_delta", "request_id": requestID, "text": String(word) + " "])
            await state.pause(0.018)
        }
        yield(["type": "reply", "request_id": requestID, "text": text, "cards": cards, "at": PlaygroundSamples.time()])
    }

    var sampleCards: [[String: Any]] { [
        ["type": "event", "kind": "reminder", "id": "sample-review", "what": "Review the launch mockups", "when": PlaygroundSamples.time(7200)],
        ["type": "event", "kind": "schedule", "id": "sample-morning", "what": "Morning brief", "when": PlaygroundSamples.next(hour: 9, weekdaysOnly: true), "repeat": "Every weekday at 9 AM"],
        ["type": "note", "item_id": "sample-memory-1", "text": "Keep morning briefs short, with decisions first."],
        ["type": "payment", "amount": "$24.00", "merchant": "Sample stationery shop", "site": "shop.example", "card": "Sample card"],
        ["type": "send", "recipient": "Alex", "app": "Sample Mail", "account": "studio@example.com"],
        ["type": "link", "host": "example.com", "title": "Sample reference"],
        ["type": "file", "path": artifact.path, "name": "Afternoon brief.txt"],
        ["type": "page", "page": "memory", "title": "What FERB remembers"]
    ] }

    func deliverReminder() {
        let id = "sample-reminder-" + UUID().uuidString
        onNotification?(["id": "reminder:" + id, "category": "reminder", "title": "Water break",
                         "body": "Time to take a break and get some water.",
                         "target": ["kind": "reminder", "timer_id": id], "when_hidden": false,
                         "card": ["type": "reminder", "title": "Water break", "kind": "reminder", "timer_id": id]])
    }

    func deliverScheduledResult() {
        let result: [String: Any] = ["task_id": "sample-scheduled-" + UUID().uuidString, "goal": "Bring me a short morning brief", "name": "Morning brief", "status": "done",
            "answer": "Your morning brief is ready: two updates need a decision, and your afternoon has room for focused work.",
            "at": PlaygroundSamples.time(), "shown": false,
            "cards": [["type": "reminder", "title": "Morning brief", "due": PlaygroundSamples.time(),
                       "repeat": "Weekdays at 9:00 AM", "schedule_id": "sample-morning",
                       "next_run": PlaygroundSamples.next(hour: 9, weekdaysOnly: true)]]]
        tasks.insert(result, at: 0)
        emit("task.result", result)
    }
}
#endif
