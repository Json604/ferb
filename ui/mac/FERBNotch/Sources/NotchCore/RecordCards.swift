import Foundation

// The rows of FERB's record cards (ORCH-563..ORCH-565, DMN-554): what a reminders, memory, permissions, logins, about or
// task card lists, read from the daemon's payload and from the saved chat (APP-801: only a row's identity keys are
// required; any other key may be missing and reads as empty). Nothing here holds a secret's value, a username or an
// account: the logins rows carry names and sites only.

/// One pending reminder or timer on a reminders card: its id, kind (reminder | timer), short name and when it is due.
public struct ReminderListRow: Codable, Equatable {
    public let id: String
    public let name: String
    public let kind: String
    public let at: String?

    public init(id: String, name: String, kind: String, at: String?) {
        self.id = id; self.name = name; self.kind = kind; self.at = at
    }

    public init?(_ value: Any?) {
        guard let value = value as? [String: Any], let id = value["id"] as? String else { return nil }
        let name = (value["name"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        self.init(id: id, name: name ?? "Reminder", kind: value["kind"] as? String ?? "reminder", at: value["at"] as? String)
    }

    public var atDate: Date? { LiveProgress.time(at) }
    public var isTimer: Bool { kind == "timer" }

    private enum CodingKeys: String, CodingKey { case id, name, kind, at }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.value(.name, or: "Reminder")
        kind = try c.value(.kind, or: "reminder")
        at = try c.decodeIfPresent(String.self, forKey: .at)
    }
}

/// A question a task asked the owner and the answer they gave (from its owner.ask records).
public struct AnsweredQuestion: Codable, Equatable {
    public let question: String
    public let answer: String

    public init?(_ value: Any?) {
        guard let value = value as? [String: Any], let question = value["question"] as? String,
              let answer = value["answer"] as? String else { return nil }
        self.question = question; self.answer = answer
    }
}

/// One thing FERB remembers on a memory card: its words (cut by the daemon), its kind and where it came from.
public struct MemoryListRow: Codable, Equatable {
    public let id: String?
    public let text: String
    public let kind: String?
    public let subject: String?
    public let origin: String?

    public init?(_ value: Any?) {
        guard let value = value as? [String: Any], let text = value["text"] as? String else { return nil }
        self.text = text
        id = value["id"] as? String; kind = value["kind"] as? String; subject = value["subject"] as? String
        origin = value["origin"] as? String
    }

    /// The owner said it (owner_stated) or FERB inferred it; nil when the row doesn't say.
    public var saidByOwner: Bool? {
        switch origin {
        case "owner_stated": return true
        case "inferred": return false
        default: return nil
        }
    }

    private enum CodingKeys: String, CodingKey { case id, text, kind, subject, origin }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        text = try c.decode(String.self, forKey: .text)
        id = try c.decodeIfPresent(String.self, forKey: .id)
        kind = try c.decodeIfPresent(String.self, forKey: .kind)
        subject = try c.decodeIfPresent(String.self, forKey: .subject)
        origin = try c.decodeIfPresent(String.self, forKey: .origin)
    }
}

/// A forget on record: when, and how many items went. Their contents are never kept.
public struct ForgetRow: Codable, Equatable {
    public let at: String
    public let items: Int

    public init?(_ value: Any?) {
        guard let value = value as? [String: Any], let at = value["at"] as? String else { return nil }
        self.at = at; items = value["items"] as? Int ?? 0
    }

    public var atDate: Date? { LiveProgress.time(at) }

    private enum CodingKeys: String, CodingKey { case at, items }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        at = try c.decode(String.self, forKey: .at)
        items = try c.value(.items, or: 0)
    }
}

/// A standing send scope in force: what kind of send, as whom, where and to whom FERB may send without asking, and
/// whether any text is allowed; `heldBy` is the task or schedule holding it.
public struct StandingSend: Codable, Equatable {
    public let heldBy: String?
    public let capability: String?
    public let identity: String?
    public let account: String?
    public let site: String?
    public let recipients: [String]
    public let anyText: Bool

    public init?(_ value: Any?) {
        guard let value = value as? [String: Any] else { return nil }
        func words(_ key: String) -> String? { (value[key] as? String).flatMap { $0.isEmpty ? nil : $0 } }
        heldBy = words("held_by"); capability = words("capability"); identity = words("identity")
        account = words("account"); site = words("site")
        recipients = (value["recipients"] as? [Any])?.compactMap { $0 as? String } ?? []
        anyText = value["any_text"] as? Bool ?? false
    }

    private enum CodingKeys: String, CodingKey { case heldBy, capability, identity, account, site, recipients, anyText }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        heldBy = try c.decodeIfPresent(String.self, forKey: .heldBy)
        capability = try c.decodeIfPresent(String.self, forKey: .capability)
        identity = try c.decodeIfPresent(String.self, forKey: .identity)
        account = try c.decodeIfPresent(String.self, forKey: .account)
        site = try c.decodeIfPresent(String.self, forKey: .site)
        recipients = try c.value(.recipients, or: [])
        anyText = try c.value(.anyText, or: false)
    }
}

/// A destination FERB knows: one the owner named in a request (`named`), chose in an answer (`answered`) or confirmed
/// (`confirmed`); the last two count as approved. A digest-kept one says so in its value.
public struct KnownDestination: Codable, Equatable {
    public let value: String
    public let kind: String?
    public let how: String?
    public let approved: Bool

    public init?(_ value: Any?) {
        guard let row = value as? [String: Any], let shown = row["value"] as? String else { return nil }
        self.value = shown
        kind = row["kind"] as? String; how = row["how"] as? String; approved = row["approved"] as? Bool ?? false
    }

    private enum CodingKeys: String, CodingKey { case value, kind, how, approved }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        value = try c.decode(String.self, forKey: .value)
        kind = try c.decodeIfPresent(String.self, forKey: .kind)
        how = try c.decodeIfPresent(String.self, forKey: .how)
        approved = try c.value(.approved, or: false)
    }
}

/// One of the owner's saved logins or card entries: its place (a site) and name, its kind, whether it is unusable; or
/// the problem reading them. Never a value or a username.
public struct SavedLogin: Codable, Equatable {
    public let place: String?
    public let name: String?
    public let kind: String?
    public let unusable: Bool
    public let problem: String?

    public init?(_ value: Any?) {
        guard let value = value as? [String: Any] else { return nil }
        place = value["place"] as? String; name = value["name"] as? String; kind = value["kind"] as? String
        unusable = value["unusable"] as? Bool ?? false; problem = value["problem"] as? String
        if name == nil && problem == nil { return nil }
    }

    private enum CodingKeys: String, CodingKey { case place, name, kind, unusable, problem }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        place = try c.decodeIfPresent(String.self, forKey: .place)
        name = try c.decodeIfPresent(String.self, forKey: .name)
        kind = try c.decodeIfPresent(String.self, forKey: .kind)
        unusable = try c.value(.unusable, or: false)
        problem = try c.decodeIfPresent(String.self, forKey: .problem)
    }
}

/// One of FERB's own secrets: its name and the sites it may be filled on. The account hint the daemon sends is not
/// kept (names and sites only).
public struct OwnLogin: Codable, Equatable {
    public let name: String
    public let sites: [String]

    public init?(_ value: Any?) {
        guard let value = value as? [String: Any], let name = value["name"] as? String else { return nil }
        self.name = name
        sites = (value["sites"] as? [Any])?.compactMap { $0 as? String } ?? []
    }

    private enum CodingKeys: String, CodingKey { case name, sites }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decode(String.self, forKey: .name)
        sites = try c.value(.sites, or: [])
    }
}

/// The model one of FERB's roles uses ("planner": "Codex gpt-6").
public struct ModelRow: Codable, Equatable {
    public let role: String
    public let model: String

    public init(role: String, model: String) { self.role = role; self.model = model }

    /// The daemon's {role: model} as rows: the planner, worker and checker first, then any other in name order.
    public static func rows(_ value: [String: Any]) -> [ModelRow] {
        let order = ["planner", "worker", "checker"]
        return value.compactMap { key, model in (model as? String).map { ModelRow(role: key, model: $0) } }
            .sorted { (order.firstIndex(of: $0.role) ?? order.count, $0.role) < (order.firstIndex(of: $1.role) ?? order.count, $1.role) }
    }
}

/// A permission's state as the daemon reads it (granted, denied, not_determined, unknown).
public struct PermissionStateRow: Codable, Equatable {
    public let id: String
    public let status: String

    public init?(_ value: Any?) {
        guard let value = value as? [String: Any], let id = value["id"] as? String else { return nil }
        self.id = id; status = value["status"] as? String ?? "unknown"
    }

    public var granted: Bool { status == "granted" }

    private enum CodingKeys: String, CodingKey { case id, status }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        status = try c.value(.status, or: "unknown")
    }
}

extension ResultCard {
    /// A record card as one short line inside a task card (its icon and words), from the card's own fields; nil for a
    /// card that isn't a record of something the task did.
    public var recordLine: (icon: String, text: String)? {
        func file() -> String { name ?? path.map { ($0 as NSString).lastPathComponent } ?? "a file" }
        switch type {
        case "send":
            let verb = ["call": "Called", "mailbox_send": "Emailed", "post": "Posted", "reply": "Replied to"][kind ?? ""] ?? "Sent to"
            let place = site ?? app
            if let recipient { return ("paperplane", verb + " " + recipient + (place.map { " · " + $0 } ?? "")) }
            return ("paperplane", (kind == "post" ? "Posted" : "Sent") + (place.map { " on " + $0 } ?? ""))
        case "payment":
            let at = merchant ?? site
            return ("creditcard", "Paid" + (amount.map { " " + $0 } ?? "") + (at.map { " at " + $0 } ?? ""))
        case "submit": return ("checkmark.rectangle", "Submitted a form on " + (site ?? "a site"))
        case "trash": return ("trash", "Moved " + file() + " to the Trash")
        case "delete": return ("xmark.bin", "Deleted " + file() + " permanently")
        case "download": return ("arrow.down.circle", "Downloaded " + file())
        case "file": return ("doc", "Made " + file())
        case "link": return host.map { ("globe", "Visited " + $0) }
        case "note": return text.map { ("brain", "Remembered: " + $0) }
        case "event": return what.map { ("calendar", "Set: " + $0) }
        default: return nil
        }
    }

    /// A task card's status in the owner's words, from code's status (ORCH-564's TASK_STATUS_SAID).
    public static func statusSaid(_ status: String?) -> String {
        ["running": "Running", "paused": "Paused", "stopping": "Stopping", "done": "Done and checked",
         "done_unverified": "Done, not all of it confirmed", "failed": "Couldn't finish", "cancelled": "Stopped",
         "needs_user": "Waiting for your answer", "interrupted": "Interrupted by a restart"][status ?? ""]
            ?? (status ?? "Unknown").replacingOccurrences(of: "_", with: " ").capitalized
    }
}
