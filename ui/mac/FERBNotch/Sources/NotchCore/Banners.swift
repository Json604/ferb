import Foundation

/// FERB's notification categories (L34 D), named as the daemon names them; the owner switches them in Settings
/// (`notifications.categories`, config.toml [banners]).
public enum BannerCategory: String, CaseIterable {
    case question, reminder, alert, taskFinished = "task_finished", loop

    public var title: String {
        switch self {
        case .question: return "Questions while the notch is closed"
        case .reminder: return "Reminders"
        case .alert: return "Alerts from things FERB watches"
        case .taskFinished: return "Tasks finishing"
        case .loop: return "Loops pausing or backing off"
        }
    }
}

/// What clicking a notification opens in the notch.
public enum NotificationTarget: Equatable {
    case question(approvalID: String, taskID: String?)
    case task(String)
    case reminder(String)
    case handoff(String)        // a human check handed to the owner: the click brings its tab forward (2026-10-27)
    case notch

    public init(_ target: [String: Any]) {
        let kind = target["kind"] as? String
        if kind == "question", let id = target["approval_id"] as? String, !id.isEmpty {
            let task = target["task_id"] as? String
            self = .question(approvalID: id, taskID: task?.isEmpty == false ? task : nil)
        } else if kind == "task", let id = target["task_id"] as? String, !id.isEmpty {
            self = .task(id)
        } else if kind == "reminder", let id = target["timer_id"] as? String, !id.isEmpty {
            self = .reminder(id)
        } else if kind == "handoff", let id = target["handoff_id"] as? String, !id.isEmpty {
            self = .handoff(id)
        } else {
            self = .notch
        }
    }

    /// Read back from a clicked notification's userInfo.
    public init(userInfo: [AnyHashable: Any]) {
        var target: [String: Any] = [:]
        for (key, value) in userInfo { if let key = key as? String { target[key] = value } }
        self.init(target)
    }

    /// Kept in the notification's userInfo: ids only.
    public var userInfo: [String: String] {
        switch self {
        case .question(let id, let task): return ["kind": "question", "approval_id": id, "task_id": task ?? ""]
        case .task(let id): return ["kind": "task", "task_id": id]
        case .reminder(let id): return ["kind": "reminder", "timer_id": id]
        case .handoff(let id): return ["kind": "handoff", "handoff_id": id]
        case .notch: return ["kind": "notch"]
        }
    }
}

/// One banner from `notifications.subscribe`. The daemon wrote its text from redacted data; FERB.app posts it as is.
public struct BannerItem: Equatable {
    public let id: String
    public let category: BannerCategory?
    public let title: String
    public let body: String
    public let target: NotificationTarget
    public let whenHidden: Bool

    public init?(_ event: [String: Any]) {
        guard let id = event["id"] as? String, !id.isEmpty, let title = event["title"] as? String,
              let body = event["body"] as? String else { return nil }
        self.id = id
        category = BannerCategory(rawValue: event["category"] as? String ?? "")
        self.title = title
        self.body = body
        target = NotificationTarget(event["target"] as? [String: Any] ?? [:])
        whenHidden = event["when_hidden"] as? Bool ?? false
    }

    /// The item this banner shows, as the notch knows it: the id after its kind (`loop:<notice id>`, `task:<task id>`,
    /// `question:<approval id>`) and its target's id. A notch sound for one of them is the same notification's.
    public var soundKeys: [String] {
        var keys: [String] = []
        if let colon = id.firstIndex(of: ":") { keys.append(String(id[id.index(after: colon)...])) }
        switch target {
        case .question(let approvalID, _): keys.append(approvalID)
        case .task(let item), .reminder(let item), .handoff(let item): keys.append(item)
        case .notch: break
        }
        return keys
    }
}

public enum NotificationGate {
    /// A banner marked when_hidden (a question, a task's answer, a loop) posts only while the notch isn't showing: open
    /// on screen, it is already in front of the owner. Reminders and alerts always post.
    public static func shouldPost(_ item: BannerItem, notchShowing: Bool) -> Bool { !(item.whenHidden && notchShowing) }

    /// The id a question's banner has (the daemon's `question:<approval id>`), so an answer withdraws it.
    public static func questionID(_ approvalID: String) -> String { "question:" + approvalID }
}
