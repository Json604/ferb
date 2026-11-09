import Foundation

public struct DaemonApproval: Identifiable, Equatable {
    public let id: String
    public let taskID: String?
    public let kind: String
    public let reason: String
    /// FERB's own question in one line, written by the daemon's code (P27): the card's one question, carrying the facts
    /// that decide it (GATE-225: no Details; the full line with the step stays in the daemon's record).
    public let summary: String?
    /// The task's reason, shown smaller under the question (owner 2026-10-27: "just ask simply what you want with a small
    /// reason below").
    public let why: String?
    /// FERB's own question's why line (GATE-513): plain words under the question, from the daemon's code.
    public let detail: String?
    /// The exact thing the owner approves (GATE-513): a command, an address, a value, a path, an amount; the card shows
    /// it in its own code box, word for word.
    public let exact: String?
    /// A task's (model-written) question: the card draws a small "from the task" tag (D33, GATE-225). Structural,
    /// from the daemon's flag, never read from the text.
    public let fromTask: Bool
    /// The one question the card shows (item 10, 2026-11-05: "it shoudlbe just its question directly").
    public var visibleReason: String { summary.flatMap { $0.isEmpty ? nil : $0 } ?? reason }
    /// FERB's own code-written question (it has a summary, or it is a confirmation FERB's code raised): shown verbatim,
    /// never as Markdown, so a command's backticks, asterisks, brackets and backslashes read as typed (D31).
    public var plainText: Bool { !(summary ?? "").isEmpty || capability == "ferb.confirm" }
    public let capability: String?
    public let options: [String]
    /// GATE-566: each option's button words from the daemon (an action word such as "Send" for Yes); an empty or missing
    /// one shows the option itself. The answer sent is still the option's number.
    public let labels: [String]
    /// GATE-566: a line under the buttons (what the 30-minute answer covers).
    public let note: String?
    /// GATE-566: a hidden input's field placeholder, decline and send words from the daemon.
    public let fieldLabel: String?
    public let declineLabel: String?
    public let sendLabel: String?
    /// The card's header, by the question's structure (GATE-566), from the daemon; nil keeps the app's own.
    public let title: String?
    public let evidence: [String]
    /// The task id the question was asked for, before the notch filed it under its root task (`task_id` is the root):
    /// a step's task id, or a task under it, ties a confirmation to that step's row in a card (ORCH-319).
    public let sourceTaskID: String?
    /// The part of a multi-part request FERB's own question is about (ORCH-319).
    public let part: String?
    /// The offer of another AI service (`runtime.switch`, ORCH-315): answered only with its buttons.
    public let runtimeSwitch: RuntimeSwitch?
    /// The words on option `index`'s button: the daemon's label for it, else the option itself (GATE-566).
    public func optionLabel(_ index: Int) -> String {
        let label = labels.indices.contains(index) ? labels[index].trimmingCharacters(in: .whitespaces) : ""
        return label.isEmpty ? options[index] : label
    }
    public var isSecret: Bool { ["owner_login_code", "owner_admin_password", "owner_secret"].contains(kind) }
    /// FERB's own question about whether the task goes on (a check-in, "Resume?", a closed tab, a runtime offer):
    /// answered only with its buttons; the daemon refuses typed words (ORCH-536). The daemon's flag, never the text.
    public let optionsOnly: Bool
    /// A safety check that belongs to a task (a confirmation, a retry, a conversation's scope): besides its buttons, the
    /// owner's words for that task go as a correction through `chat.correct`, never as a Yes (GATE-510).
    public let corrects: Bool

    /// The card's text box (APP-726): none, an answer (a secret, a question without choices, a task's question with
    /// choices, in a row too), or the owner's words for the task beside a safety check's buttons.
    public enum AnswerBox: Equatable { case none, answer, correction }
    public var answerBox: AnswerBox {
        if isSecret || options.isEmpty { return .answer }
        if optionsOnly || runtimeSwitch != nil { return .none }
        if corrects { return .correction }
        return fromTask ? .answer : .none
    }
    public init?(_ value: [String: Any]) {
        guard let id = value["approval_id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        taskID = value["task_id"] as? String
        kind = value["kind"] as? String ?? "owner_question"
        reason = value["reason"] as? String ?? "FERB needs your answer."
        capability = value["capability"] as? String
        summary = value["summary"] as? String
        fromTask = value["from_task"] as? Bool ?? false
        why = (value["why"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        detail = (value["detail"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        exact = (value["exact"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        options = value["options"] as? [String] ?? []
        labels = value["labels"] as? [String] ?? []
        note = (value["note"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        fieldLabel = (value["field_label"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        declineLabel = (value["decline_label"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        sendLabel = (value["send_label"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        title = (value["title"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        evidence = value["evidence"] as? [String] ?? []
        sourceTaskID = value["source_task_id"] as? String ?? taskID
        part = (value["part"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        runtimeSwitch = RuntimeSwitch(value["runtime_switch"])
        optionsOnly = value["options_only"] as? Bool ?? false
        corrects = value["corrects"] as? Bool ?? false
    }
}
