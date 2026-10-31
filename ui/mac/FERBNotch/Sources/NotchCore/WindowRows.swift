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
    /// Its recorded end (decision 22); nil runs until stopped.
    public let until: Date?
    public let failures: Int
    public let lastResult: LastResult?

    public init?(_ row: [String: Any]) {
        guard let id = row["id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        request = text(row["request"])
        rule = text(row["rule"])
        state = text(row["state"])
        nextRun = WindowTime.parse(row["next_run"])
        until = WindowTime.parse(row["until"])
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

/// A memory item (`memory.list`): its words, where it came from (`label`: your words / FERB's understanding / observed,
/// notes/memory.md decision 15), what it is about, and whether it is in the core profile (always on).
public struct MemoryRow: Identifiable, Equatable {
    public let id: String
    public let kind: String
    public let subject: String
    public let key: String
    public let content: String
    public let status: String
    public let origin: String
    public let label: String
    public let about: String
    public let inProfile: Bool

    public init?(_ row: [String: Any]) {
        guard let id = row["id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        kind = text(row["kind"])
        subject = text(row["subject"])
        key = text(row["key"])
        content = text(row["content"])
        status = text(row["status"])
        origin = text(row["origin"])
        label = text(row["label"])
        about = text(row["about"])
        inProfile = flag(row["in_profile"])
    }
}

/// The core profile (`memory.profile`): its sections of memory ids in order, and its budget.
public struct CoreProfileView: Equatable {
    public struct Section: Equatable {
        public let title: String
        public let ids: [String]
    }
    public let sections: [Section]
    public let usedTokens: Int
    public let budgetTokens: Int

    public init(_ reply: [String: Any]) {
        sections = (reply["sections"] as? [[String: Any]] ?? []).map { section in
            Section(title: text(section["title"]),
                    ids: (section["items"] as? [[String: Any]] ?? []).compactMap { $0["id"] as? String })
        }
        usedTokens = reply["used_tokens"] as? Int ?? 0
        budgetTokens = reply["budget_tokens"] as? Int ?? 0
    }

    public init() { sections = []; usedTokens = 0; budgetTokens = 0 }
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

/// Where a memory import reads from (`memory.import.*`, notes/memory-import.md decision 3): three account exports the
/// owner chooses as a file or folder, and two local transcript folders FERB reads on this Mac.
public enum MemoryImportSource: String, CaseIterable, Identifiable {
    case chatgpt, claude, gemini, claude_code, codex

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .chatgpt: return "ChatGPT (data export)…"
        case .claude: return "Claude (data export)…"
        case .gemini: return "Gemini (Google Takeout)…"
        case .claude_code: return "Claude Code (this Mac)"
        case .codex: return "Codex CLI (this Mac)"
        }
    }

    /// Exports need the owner's file; the local sources are read where they live.
    public var needsFile: Bool { self == .chatgpt || self == .claude || self == .gemini }
}

/// One memory import run (`memory.import.status` `current`): its state and counts, as one line for the Memory page.
public struct MemoryImportStatus: Equatable {
    public let run: String
    public let name: String
    public let status: String
    public let error: String
    public let chunksDone: Int
    public let chunks: Int
    public let facts: Int
    public let modelCalls: Int

    public init?(_ row: [String: Any]?) {
        guard let row, let run = row["run"] as? String, !run.isEmpty else { return nil }
        self.run = run
        name = text(row["name"])
        status = text(row["status"])
        error = text(row["error"])
        let counts = row["counts"] as? [String: Any] ?? [:]
        chunksDone = counts["chunks_done"] as? Int ?? 0
        chunks = (counts["chunks_done"] as? Int ?? 0) + (counts["chunks_left"] as? Int ?? 0)
        facts = counts["facts_saved"] as? Int ?? 0
        modelCalls = counts["model_calls"] as? Int ?? 0
    }

    public var running: Bool { status == "running" }

    public var line: String {
        switch status {
        case "running": return "Importing from \(name): \(chunksDone) of \(chunks) parts read, \(facts) facts so far."
        case "done": return "Imported \(facts) facts from \(name) (\(modelCalls) model calls)."
        case "cancelled": return "Import from \(name) cancelled after \(facts) facts. Start it again to go on."
        default: return "Import from \(name) stopped: \(error.isEmpty ? status : error)."
        }
    }
}

/// What an import would read (`memory.import.estimate`), before anything is sent.
public struct MemoryImportEstimate: Equatable {
    public let source: String
    public let path: String
    public let line: String

    public init(source: String, path: String, reply: [String: Any]) {
        self.source = source
        self.path = path
        let name = text(reply["name"])
        let conversations = reply["conversations"] as? Int ?? 0
        let calls = reply["model_calls_estimate"] as? Int ?? 0
        line = "\(name): \(conversations) conversations from the last 3 months, about \(calls) model calls. "
            + "Only redacted text is sent; FERB keeps the facts, not the chats."
    }
}
