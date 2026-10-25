import Foundation

/// The FERB window's rows (L34 Phase E), read from the daemon's replies. A row the daemon sent without its id is
/// dropped; every other missing field reads as empty, so an older daemon still shows what it has.
public enum WindowTime {
    /// The daemon's times are Python `isoformat()`: an offset, and microseconds unless they are zero.
    public static func parse(_ value: Any?) -> Date? {
        guard let text = value as? String, !text.isEmpty else { return nil }
        let full = ISO8601DateFormatter()
        full.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = full.date(from: text) { return date }
        let plain = ISO8601DateFormatter()
        plain.formatOptions = [.withInternetDateTime]
        return plain.date(from: text)
    }
}

private func text(_ value: Any?) -> String { value as? String ?? "" }
private func number(_ value: Any?) -> Int { (value as? NSNumber)?.intValue ?? 0 }
private func flag(_ value: Any?) -> Bool { (value as? NSNumber)?.boolValue ?? false }

/// A root task (`tasks.list`).
public struct TaskHistoryRow: Identifiable, Equatable {
    public let id: String
    public let goal: String
    public let status: String
    public let createdAt: Date?
    /// The daemon's own `created_at`, sent back as `before` for the next page.
    public let createdAtText: String
    public let answer: String
    public let verdict: String?

    public init?(_ row: [String: Any]) {
        guard let id = row["task_id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        goal = text(row["goal"])
        status = text(row["status"])
        createdAtText = text(row["created_at"])
        createdAt = WindowTime.parse(row["created_at"])
        answer = text(row["answer"])
        verdict = row["verdict"] as? String
    }

    /// Still running: the owner can stop it; a verdict is for a task that ended.
    public var isRunning: Bool { status == "running" || status == "planned" }
}

/// A schedule (`schedules.list`) with its latest finished run.
public struct ScheduleRow: Identifiable, Equatable {
    public struct LastResult: Equatable {
        public let at: Date?
        public let status: String
        public let answer: String
    }

    public let id: String
    public let request: String
    public let rule: String
    public let state: String
    public let nextRun: Date?
    public let failures: Int
    public let lastResult: LastResult?

    public init?(_ row: [String: Any]) {
        guard let id = row["id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        request = text(row["request"])
        rule = text(row["rule"])
        state = text(row["state"])
        nextRun = WindowTime.parse(row["next_run"])
        failures = number(row["failures"])
        if let last = row["last_result"] as? [String: Any] {
            lastResult = LastResult(at: WindowTime.parse(last["at"]), status: text(last["status"]),
                                    answer: text(last["answer"]))
        } else {
            lastResult = nil
        }
    }

    /// What the owner may do now (`schedules.change` actions).
    public var actions: [String] {
        switch state {
        case "active": return ["pause", "cancel"]
        case "paused": return ["resume", "cancel"]
        default: return ["cancel"]
        }
    }
}

/// A pending reminder or timer (`reminders.list`).
public struct ReminderRow: Identifiable, Equatable {
    public let id: String
    public let kind: String
    public let at: Date?
    public let message: String

    public init?(_ row: [String: Any]) {
        guard let id = row["id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        kind = text(row["kind"])
        at = WindowTime.parse(row["at"])
        message = text(row["message"])
    }
}

/// Something FERB noticed (`notices.list`); its quotes are outside text, shown as quotes (MP2).
public struct NoticeRow: Identifiable, Equatable {
    public let id: String
    public let text: String
    public let quotes: [NoticeQuote]
    public let at: Date?
    public let shown: Bool

    public init?(_ row: [String: Any]) {
        guard let id = row["id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        self.text = row["text"] as? String ?? ""
        quotes = (row["quotes"] as? [[String: Any]] ?? []).compactMap { quote in
            guard let words = quote["text"] as? String, !words.isEmpty else { return nil }
            return NoticeQuote(text: words, origin: quote["origin"] as? String ?? "")
        }
        at = WindowTime.parse(row["at"])
        shown = flag(row["shown"])
    }
}

/// A memory item (`memory.list`).
public struct MemoryRow: Identifiable, Equatable {
    public let id: String
    public let kind: String
    public let subject: String
    public let key: String
    public let content: String
    public let status: String
    public let origin: String

    public init?(_ row: [String: Any]) {
        guard let id = row["id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        kind = text(row["kind"])
        subject = text(row["subject"])
        key = text(row["key"])
        content = text(row["content"])
        status = text(row["status"])
        origin = text(row["origin"])
    }
}

/// One setting's value (`settings.get`). JSON's booleans are told apart from its numbers.
public enum SettingValue: Equatable {
    case bool(Bool), int(Int), string(String), strings([String])

    public init?(_ any: Any) {
        switch any {
        case let words as String: self = .string(words)
        case let list as [String]: self = .strings(list)
        case let n as NSNumber where CFGetTypeID(n) == CFBooleanGetTypeID(): self = .bool(n.boolValue)
        case let n as NSNumber: self = .int(n.intValue)
        default: return nil
        }
    }

    public var json: Any {
        switch self {
        case .bool(let value): return value
        case .int(let value): return value
        case .string(let value): return value
        case .strings(let value): return value
        }
    }
}

public struct SettingsSnapshot: Equatable {
    public var values: [String: SettingValue]

    public init(values: [String: SettingValue] = [:]) { self.values = values }

    public init(_ reply: [String: Any]) {
        values = (reply["values"] as? [String: Any] ?? [:]).compactMapValues(SettingValue.init)
    }

    public func bool(_ key: String) -> Bool { if case .bool(let v)? = values[key] { return v }; return false }
    public func int(_ key: String) -> Int { if case .int(let v)? = values[key] { return v }; return 0 }
    public func strings(_ key: String) -> [String] { if case .strings(let v)? = values[key] { return v }; return [] }
}

/// One line of Settings → Advanced → Check FERB (`doctor.check` with its self-test).
public struct HealthLine: Identifiable, Equatable {
    public var id: String { name }
    public let name: String
    public let ok: Bool
    public let detail: String

    public static func lines(_ doctor: [String: Any]) -> [HealthLine] {
        var out = (doctor["selftest"] as? [[String: Any]] ?? []).compactMap { row -> HealthLine? in
            guard let name = row["name"] as? String else { return nil }
            return HealthLine(name: name, ok: flag(row["ok"]), detail: text(row["detail"]))
        }
        if let containment = doctor["containment"] as? [String: Any] {
            let ok = flag(containment["profile_ok"]) && flag(containment["proxy_alive"])
            let reason = text(containment["reason"])
            out.append(HealthLine(name: "Shell containment", ok: ok,
                                  detail: ok ? "" : (reason.isEmpty ? "sandbox or proxy down" : reason)))
        }
        return out
    }
}
