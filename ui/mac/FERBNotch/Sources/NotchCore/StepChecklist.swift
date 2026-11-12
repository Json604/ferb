import Foundation

/// A plan's steps as checklist lines (APP-894, APP-895; daemon ORCH-626): a single ask of three or more steps shows its
/// steps on its card, and a multi-part card's row shows the steps it waits on that answer no row. Pure state: the
/// notch draws it (StepChecklistView, TurnCardView's feeder lines).

/// One plan step as the daemon names it (`checklist.steps`, a card's `plan`): its id, its words (the planner's label,
/// else its goal's first clause, cut), its kind, what it waits on, its step task id (its live line, its preview, its
/// questions) and whether it repeats.
public struct PlanStep: Codable, Equatable, Identifiable {
    public let id: String
    public let label: String
    public let kind: String
    public let dependsOn: [String]
    public let stepTaskID: String
    public let loop: Bool

    public init?(_ value: Any?) {
        guard let value = value as? [String: Any], let id = value["id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        label = (value["label"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? id
        kind = value["kind"] as? String ?? "do"
        dependsOn = value["depends_on"] as? [String] ?? []
        stepTaskID = value["step_task_id"] as? String ?? ""
        loop = value["loop"] as? Bool ?? false
    }

    private enum CodingKeys: String, CodingKey { case id, label, kind, dependsOn, stepTaskID, loop }

    /// Read tolerantly (APP-801): only its id is required.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        label = try c.value(.label, or: id)
        kind = try c.value(.kind, or: "do")
        dependsOn = try c.value(.dependsOn, or: [])
        stepTaskID = try c.value(.stepTaskID, or: "")
        loop = try c.value(.loop, or: false)
    }

    public static func list(_ value: Any?) -> [PlanStep] { (value as? [Any] ?? []).compactMap(PlanStep.init) }
}

/// A single ask's checklist (APP-894): its plan's steps, their live states while the task runs, and, once it ended,
/// what each step ended as. The steps and the end are saved with the chat, so a restored chat shows the collapsed line
/// and its steps as they ended; live states never are.
public struct StepChecklist: Codable, Equatable {
    public let taskID: String
    public private(set) var steps: [PlanStep]
    /// Live state only (ORCH-531): never saved.
    public var states: [String: StepState] = [:]
    /// What each step ended as (`done`, `failed`, `not_finished`, `wont_run`, `stopped`), from the outcome's record.
    public private(set) var ended: [String: String] = [:]
    /// The task ended: the checklist collapses to its one line.
    public private(set) var finished = false

    private enum CodingKeys: String, CodingKey { case taskID, steps, ended, finished }

    public init?(_ payload: Any?, taskID: String) {
        guard let payload = payload as? [String: Any] else { return nil }
        let steps = PlanStep.list(payload["steps"])
        guard !steps.isEmpty else { return nil }
        self.taskID = taskID
        self.steps = steps
        for (key, value) in payload["states"] as? [String: Any] ?? [:] {
            if let state = StepState(value) { states[key] = state }
        }
    }

    /// Read tolerantly (APP-801): only its task is required; a step that can't be read is left out.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        taskID = try c.decode(String.self, forKey: .taskID)
        steps = try c.lossyList(PlanStep.self, .steps) ?? []
        ended = try c.value(.ended, or: [:])
        finished = try c.value(.finished, or: false)
    }

    /// Applies a `step` event (ORCH-531) when it is newer than the step's state shown. Returns whether it applied.
    @discardableResult
    public mutating func apply(step event: [String: Any]) -> Bool {
        guard !finished, let key = event["step"] as? String, steps.contains(where: { $0.id == key }),
              let state = StepState(event), StepState.replaces(state, states[key]), states[key] != state else { return false }
        states[key] = state
        return true
    }

    /// A newer picture of the same checklist (the plan event again, `chat.status`): its steps, and each state that is
    /// newer than the one shown.
    public mutating func merge(_ snapshot: StepChecklist) {
        steps = snapshot.steps
        for (key, state) in snapshot.states where StepState.replaces(state, states[key]) { states[key] = state }
    }

    /// The task's end (the outcome's checklist): what each step ended as, kept; the live states go.
    public mutating func end(_ snapshot: StepChecklist?) {
        if let snapshot { steps = snapshot.steps }
        let final = snapshot?.states ?? states
        ended = final.mapValues(\.status).filter { ["done", "failed", "not_finished", "wont_run", "stopped"].contains($0.value) }
        states = [:]
        finished = true
    }

    /// Steps that ended done, for the count ("2 of 4").
    public func doneCount(live: Bool) -> Int {
        steps.filter { (live ? states[$0.id]?.status : nil) ?? ended[$0.id] == "done" }.count
    }
}

/// What one checklist line shows: its look (the card rows' states, APP-718, ORCH-532, ORCH-613, plus `upcoming`: not
/// started, waiting on an earlier step), the words after its label and the spinner while it works.
public struct ChecklistLine: Equatable, Identifiable {
    public enum Kind: String {
        case done, running, upcoming, waiting, needsYou, failed, wontRun, stopped, notFinished, loop
    }

    public let step: PlanStep
    public let kind: Kind
    public let words: String?
    public let spinner: Bool
    public var id: String { step.id }

    /// Working now: its live preview and its spinner show (APP-888).
    public var isRunning: Bool { kind == .running || kind == .loop }

    /// How VoiceOver reads its state.
    public var stateName: String {
        switch kind {
        case .done: return "done"
        case .running, .loop: return "running"
        case .upcoming: return "not started"
        case .waiting: return "waiting"
        case .needsYou: return "asking you"
        case .failed: return "couldn't do"
        case .wontRun: return "won't run"
        case .stopped: return "stopped by you"
        case .notFinished: return "not finished"
        }
    }

    /// A line from its step's state. `live`: the task still runs (history and a dead task never show live states);
    /// `ended`: what the record says it ended as; `liveLine`: its step's own live line (`task.progress`, a sleep's
    /// "waiting until …"); `asking`: an open question names its step task; `loop`: its loop's round.
    public static func make(step: PlanStep, state: StepState?, ended: String?, live: Bool, liveLine: LiveProgress? = nil,
                            asking: Bool = false, loop: LoopRound? = nil, names: [String: String] = [:]) -> ChecklistLine {
        func line(_ kind: Kind, _ words: String? = nil, spinner: Bool = false) -> ChecklistLine {
            ChecklistLine(step: step, kind: kind, words: words, spinner: spinner)
        }
        let status = live ? (state?.status ?? ended) : ended
        switch status {
        case "done": return line(.done)
        case "failed": return line(.failed, "Couldn't do")
        case "wont_run": return line(.wontRun, "Won't run: an earlier step couldn't be done")
        case "stopped": return line(.stopped, "Stopped by you")
        case "not_finished": return line(.notFinished, "Not finished")
        default: break
        }
        guard live else { return line(.notFinished, "Not finished") }
        if asking || status == "needs_you" { return line(.needsYou, "Asking you") }
        guard let state else { return line(.upcoming) }
        if state.status == "waiting" {
            // a wait step's own time or event; a step waiting on earlier ones is simply not started yet
            if let until = state.until { return line(.waiting, "Waiting until " + RowDisplay.clock(until)) }
            if state.event != nil { return line(.waiting, "Waiting for its event") }
            return line(.upcoming)
        }
        let shown = RowDisplay.forStep(state, waitingFor: [], liveLine: liveLine, loop: loop)
        switch shown.kind {
        case .loop: return line(.loop, shown.words)
        case .needsYou: return line(.needsYou, "Asking you")
        case .notFinished: return line(.notFinished, shown.words)
        case .wontRun: return line(.wontRun, shown.words)
        default: return line(.running, shown.words, spinner: shown.spinner)
        }
    }
}

/// The checklist's count and its one line once the task ended (APP-894).
public enum ChecklistSummary {
    /// "2 of 4", top right while it runs.
    public static func count(done: Int, total: Int) -> String { "\(done) of \(total)" }

    /// "4 steps done", or "2 of 4 steps done" when it didn't finish.
    public static func collapsed(done: Int, total: Int) -> String {
        done == total ? "\(total) step\(total == 1 ? "" : "s") done" : "\(done) of \(total) steps done"
    }

    /// The steps a row's answer step waits on that answer no row of the card (APP-895): its dependencies, and theirs,
    /// in plan order, leaving out a step another row answers (that row shows it).
    public static func feeders(of step: String?, plan: [PlanStep], rowSteps: Set<String>) -> [PlanStep] {
        guard let step else { return [] }
        let byID = Dictionary(plan.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        var seen = Set<String>(), pending = byID[step]?.dependsOn ?? []
        while let next = pending.popLast() {
            guard !rowSteps.contains(next), seen.insert(next).inserted else { continue }
            pending += byID[next]?.dependsOn ?? []
        }
        return plan.filter { seen.contains($0.id) }
    }
}
