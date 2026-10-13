import Foundation

public struct DaemonApproval: Identifiable, Equatable {
    public let id: String
    public let taskID: String?
    public let kind: String
    public let reason: String
    public let displayText: String?
    public var visibleReason: String { displayText.flatMap { $0.isEmpty ? nil : $0 } ?? reason }
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
        options = value["options"] as? [String] ?? []
        title = value["title"] as? String
        evidence = value["evidence"] as? [String] ?? []
    }
}
