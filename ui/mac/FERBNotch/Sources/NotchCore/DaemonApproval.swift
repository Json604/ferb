import Foundation

public struct DaemonApproval: Identifiable, Equatable {
    public let id: String
    public let taskID: String?
    public let kind: String
    public let reason: String
    /// A worker's question reworded by the orchestrator: accepted only for an ordinary `owner.ask` question (D33).
    public let displayText: String?
    /// FERB's own question in one line, written by the daemon's code (P27): shown first, the exact reason under Details.
    public let summary: String?
    public var visibleReason: String {
        summary.flatMap { $0.isEmpty ? nil : $0 } ?? displayText.flatMap { $0.isEmpty ? nil : $0 } ?? reason
    }
    /// The full question under the "Details" disclosure, when the card shows a shorter line first.
    public var details: String? { visibleReason == reason ? nil : reason }
    public let options: [String]
    public let title: String?
    public let evidence: [String]
    public var isSecret: Bool { ["owner_login_code", "owner_admin_password", "owner_secret"].contains(kind) }
    public init?(_ value: [String: Any]) {
        guard let id = value["approval_id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        taskID = value["task_id"] as? String
        kind = value["kind"] as? String ?? "owner_question"
        reason = value["reason"] as? String ?? "FERB needs your answer."
        let ordinaryQuestion = kind == "owner_question" && value["capability"] as? String == "owner.ask"
        displayText = ordinaryQuestion ? value["display_text"] as? String : nil
        summary = value["summary"] as? String
        options = value["options"] as? [String] ?? []
        title = value["title"] as? String
        evidence = value["evidence"] as? [String] ?? []
    }
}
