import Foundation

// Step 12c (DMN-6, APP-1, APP-2, STORE-402): the models behind Settings → About, Settings →
// Data (Restore, saved secrets and cards, the one-time clean-up) and FERB's own account. Each is read from a daemon reply
// or a `ferb` command's JSON; nothing here decides anything. Functional only: the owner polishes the screens later.

private func text(_ value: Any?) -> String { value as? String ?? "" }
private func number(_ value: Any?) -> Int { (value as? NSNumber)?.intValue ?? 0 }
private func flag(_ value: Any?) -> Bool { (value as? NSNumber)?.boolValue ?? false }

/// Bytes as the owner reads them ("1.7 GB").
public func shownBytes(_ bytes: Int) -> String {
    ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)
}

// MARK: - The app guide (DMN-6: "a small list kept with the UI")

/// FERB.app's how-tos, shortcuts and where each setting lives, kept here beside the screens they describe. FERB.app
/// hands it to the daemon (`about.guide`) on each connection, so FERB's self-description and its workers say what this
/// app actually shows. Change it together with the screen it names.
public enum AppGuide {
    public struct HowTo: Equatable { public let title: String; public let steps: String }

    /// A shortcut, declared once: Settings → Notch & Shortcuts, Settings → About and FERB's guide all read it. A
    /// shortcut the owner can change names the declared setting (`setting`, an AppSettings key) whose current value
    /// is its keys; until one exists its keys are the fixed `standard` ones.
    public struct Shortcut {
        public let action: String
        public let standard: String
        public let setting: String?
        public let inNotchSettings: Bool

        public init(action: String, standard: String, setting: String? = nil, inNotchSettings: Bool = true) {
            self.action = action
            self.standard = standard
            self.setting = setting
            self.inNotchSettings = inNotchSettings
        }

        /// Its keys now: the declared setting's current value, else the standard keys.
        public func keys(_ defaults: UserDefaults = .standard) -> String {
            guard let key = setting, let declared = AppSettings.all.first(where: { $0.keys.contains(key) }) else {
                return standard
            }
            let value = declared.read(defaults)
            return value.isEmpty ? standard : value
        }
    }

    public static let shortcuts: [Shortcut] = [
        Shortcut(action: "Open / close FERB", standard: "Double-tap right Option"),
        Shortcut(action: "Dictation", standard: "Hold right Command + Option"),
        Shortcut(action: "Submit / new line", standard: "Return / Shift–Return"),
        Shortcut(action: "FERB's window and Settings", standard: "FERB's menu bar icon → Open FERB Window, or ⌘, in it",
                 inNotchSettings: false),
    ]

    public static let howtos: [HowTo] = [
        HowTo(title: "Talk to FERB",
              steps: "Type in the notch, or hold right Command + Option to dictate. FERB always replies in text in the chat; it never speaks aloud"),
        HowTo(title: "Stop everything FERB is doing", steps: "FERB window → Tasks → Stop everything (Resume FERB undoes it)"),
        HowTo(title: "Stop one task", steps: "FERB window → Tasks → the task → Stop"),
        HowTo(title: "See or change what FERB remembers", steps: "FERB window → Memory: Always on, Not right, Forget…"),
        HowTo(title: "Watch FERB work", steps: "In the notch, click a circular task preview beside the active tasks row"),
        HowTo(title: "Import memory from ChatGPT, Claude, Gemini, Claude Code or Codex",
              steps: "FERB window → Memory → Import from other assistants"),
        HowTo(title: "Change schedules and reminders", steps: "FERB window → Schedules & Reminders"),
        HowTo(title: "Choose the AI FERB uses", steps: "Settings → AI"),
        HowTo(title: "Grant or check permissions", steps: "Settings → Permissions"),
        HowTo(title: "Connect a browser profile", steps: "Settings → Browsers → Add to… / Pair"),
        HowTo(title: "Give FERB its own Google account", steps: "Settings → Browsers → FERB's own account"),
        HowTo(title: "Choose which notifications FERB posts", steps: "Settings → Sounds & Notifications"),
        HowTo(title: "Make FERB ignore an app or a site", steps: "Settings → Privacy → Apps / Sites FERB ignores"),
        HowTo(title: "Pause recording what happens on this Mac", steps: "Settings → Privacy → Pause recording"),
        HowTo(title: "Change how long FERB keeps records", steps: "Settings → Privacy → How long FERB keeps records"),
        HowTo(title: "Put back a file FERB changed or moved to the Trash", steps: "Settings → Data → Restore"),
        HowTo(title: "Delete a saved secret or card", steps: "Settings → Data → Saved secrets and cards"),
        HowTo(title: "Clean up data FERB no longer uses", steps: "Settings → Data → Clean up"),
        HowTo(title: "Check FERB's health or send diagnostics", steps: "Settings → Advanced → Health / Diagnostic file"),
        HowTo(title: "Uninstall FERB", steps: "Settings → Advanced → Uninstall FERB…"),
        HowTo(title: "See what FERB is and what it records about you", steps: "Settings → About"),
    ]

    /// Where each setting the daemon lists (`settings.get`) is changed in this app.
    public static let places: [String: String] = [
        "privacy.exclude_apps": "Settings → Privacy → Apps FERB ignores",
        "privacy.exclude_domains": "Settings → Privacy → Sites FERB ignores",
        "sense.window_titles": "Settings → Privacy → Record window titles",
        "sense.trail_retention_days": "Settings → Privacy → How long FERB keeps records → Activity trail",
        "retention.trace_days": "Settings → Privacy → How long FERB keeps records → Task traces",
        "retention.decision_days": "Settings → Privacy → How long FERB keeps records → Jev decisions",
        "retention.evidence_days": "Settings → Privacy → How long FERB keeps records → Screenshots and evidence",
        "retention.recovery_days": "Settings → Privacy → How long FERB keeps records → Recovery copies",
        "retention.resource_days": "Settings → Privacy → How long FERB keeps records → Resource log",
        "retention.operational_days": "Settings → Privacy → How long FERB keeps records → Operational records",
        "browsing.owner_profile": "Settings → Browsers → FERB's own account (when FERB has none)",
    ]

    /// The keys of one shortcut, by its action, as they are now.
    public static func keys(_ action: String, _ defaults: UserDefaults = .standard) -> String {
        shortcuts.first { $0.action == action }?.keys(defaults) ?? ""
    }

    /// `about.guide`'s parameters: the how-tos, the shortcuts with their keys now, where each daemon setting lives,
    /// and FERB.app's own settings with their current values (`AppSettings.current`).
    public static func payload(_ defaults: UserDefaults = .standard) -> [String: Any] {
        ["howtos": howtos.map { ["title": $0.title, "steps": $0.steps] },
         "shortcuts": shortcuts.map { ["action": $0.action, "keys": $0.keys(defaults)] },
         "places": places,
         "app_settings": AppSettings.current(defaults)]
    }
}

// MARK: - About (DMN-6)

/// `about.get`: FERB's self-description, as the daemon built it from code and config.
public struct AboutDocument: Equatable {
    public struct Line: Equatable, Identifiable {
        public var id: String { name + "\u{0}" + value }
        public let name: String
        public let value: String
        public let note: String
    }

    public let version: String
    public let models: [Line]
    public let account: [Line]
    public let permissions: [Line]
    public let settings: [Line]
    /// What FERB keeps: `name` what, `value` how long, `note` where.
    public let records: [Line]
    public let health: [Line]
    public let cost: [Line]
    /// FERB.app's own settings as FERB knows them now (APP-728: the guide the app sent, with current values).
    public let appSettings: [Line]

    public init(_ reply: [String: Any]) {
        version = text((reply["version"] as? [String: Any])?["ferb"])
        models = ((reply["models"] as? [String: Any]) ?? [:]).sorted { $0.key < $1.key }
            .map { Line(name: $0.key, value: text($0.value), note: "") }
        let account = reply["account"] as? [String: Any] ?? [:]
        let email = text(account["email"])
        self.account = email.isEmpty ? [Line(name: "FERB's own account", value: "not set up", note: text(account["note"]))]
            : [Line(name: "FERB's own account", value: email,
                    // its profile by name when the daemon read it (HAND-424); the account is the value beside it
                    note: [text(account["browser"]), text(account["profile_name"]).isEmpty ? text(account["profile"])
                           : "“\(text(account["profile_name"]))”"].filter { !$0.isEmpty }.joined(separator: " / ")
                        + (flag(account["paired"]) ? " · paired" : " · not paired"))]
        permissions = (reply["permissions"] as? [[String: Any]] ?? []).map {
            Line(name: text($0["id"]).replacingOccurrences(of: "_", with: " "), value: text($0["status"]), note: "")
        }
        settings = (reply["settings"] as? [[String: Any]] ?? []).map {
            Line(name: text($0["key"]), value: Self.shown($0["value"]), note: text($0["where"]))
        }
        records = (reply["records"] as? [[String: Any]] ?? []).map {
            Line(name: text($0["records"]), value: text($0["kept"]), note: text($0["where"]))
        }
        let health = reply["health"] as? [String: Any] ?? [:]
        var lines: [Line] = []
        if let bytes = health["database_bytes"] as? NSNumber {
            lines.append(Line(name: "Database", value: shownBytes(bytes.intValue),
                              note: flag(health["cleanup_waiting_for_ok"]) ? "a one-time clean-up waits for your OK" : ""))
        }
        if let cpu = health["cpu_share_last_day"] as? NSNumber {
            lines.append(Line(name: "CPU, last day", value: String(format: "%.1f%% of one core", cpu.doubleValue * 100),
                              note: ""))
        }
        if let memory = health["memory_now_bytes"] as? NSNumber {
            lines.append(Line(name: "Memory now", value: shownBytes(memory.intValue), note: ""))
        }
        self.health = lines
        let cost = reply["cost"] as? [String: Any] ?? [:]
        self.cost = [7, 30].compactMap { days -> Line? in
            guard let usd = cost["last_\(days)_days_usd"] as? NSNumber else { return nil }
            return Line(name: "Last \(days) days", value: String(format: "$%.2f", usd.doubleValue), note: text(cost["basis"]))
        }
        appSettings = ((reply["app"] as? [String: Any])?["settings"] as? [[String: Any]] ?? []).map {
            Line(name: text($0["name"]), value: text($0["value"]), note: text($0["where"]))
        }
    }

    static func shown(_ value: Any?) -> String {
        if let list = value as? [Any] { return list.isEmpty ? "none" : list.map { "\($0)" }.joined(separator: ", ") }
        if let number = value as? NSNumber {
            return CFGetTypeID(number) == CFBooleanGetTypeID() ? (number.boolValue ? "on" : "off") : number.stringValue
        }
        let words = text(value)
        return words.isEmpty ? "not set" : words
    }
}

// MARK: - Restore (APP-2)

/// One thing FERB can put back (`recovery.list`): a copy of a file before FERB changed it, an item it moved to the
/// Trash, or a disk snapshot (restored from macOS Recovery or Time Machine, not here).
public struct RestoreRow: Identifiable, Equatable {
    public let id: String
    public let kind: String
    public let original: String
    public let reason: String
    public let at: Date?
    public let savedTo: String

    public init?(_ row: [String: Any]) {
        guard let id = row["id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        kind = text(row["kind"])
        original = text(row["original"])
        reason = text(row["reason"])
        at = WindowTime.parse(row["at"])
        savedTo = text(row["saved_to"])
    }

    /// A snapshot is restored outside FERB; the rest have a Restore button.
    public var restorable: Bool { kind != "snapshot" }

    public var what: String {
        switch kind {
        case "trash": return "Moved to the Trash"
        case "snapshot": return "Disk snapshot"
        default: return reason == "overwrite" ? "Overwritten" : reason == "edit" ? "Edited" : "Changed"
        }
    }
}

// MARK: - Saved secrets and cards (APP-2)

/// One saved entry (`secrets.saved`): where it fills and its name. Never a value.
public struct SavedSecretRow: Identifiable, Equatable {
    public var id: String { place + "\u{0}" + name }
    public let place: String
    public let name: String
    public let kind: String
    public let unusable: Bool

    public init?(_ row: [String: Any]) {
        guard let place = row["place"] as? String, let name = row["name"] as? String, !name.isEmpty else { return nil }
        self.place = place
        self.name = name
        kind = text(row["kind"])
        unusable = flag(row["unusable"])
    }

    public var kindLabel: String {
        switch kind {
        case "card_number": return "card number"
        case "card_csc": return "card security code"
        case "card_expiry": return "card expiry"
        default: return "secret"
        }
    }
}

// MARK: - What FERB saw (step 12e)

/// One screenshot FERB kept (`seen.list`): the owner's front window as a message was sent, or an image a task captured,
/// stored redacted exactly as the model got it. Its bytes come from `seen.image` (FERB.app only).
public struct SeenCapture: Identifiable, Equatable {
    public let id: String
    public let at: Date?
    public let kind: String
    public let taskID: String
    public let app: String
    public let site: String
    public let title: String
    public let tool: String

    public init?(_ row: [String: Any]) {
        guard let id = row["id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        at = WindowTime.parse(row["at"])
        kind = text(row["kind"])
        taskID = text(row["task_id"])
        app = text(row["app"])
        site = text(row["site"])
        title = text(row["title"])
        tool = text(row["tool"])
    }

    /// What it shows, in a few words: the site, else the app, and the page or window title when there is one.
    public var label: String {
        let place = !site.isEmpty ? site : (!app.isEmpty ? app : what)
        return title.isEmpty || title == place ? place : "\(place) — \(title)"
    }

    /// Which capture it was.
    public var what: String {
        switch kind {
        case "front_window": return "Your front window when you sent it"
        case "app_window": return "An app window FERB looked at"
        case "snapshot": return "A page FERB read"
        default: return "A screenshot FERB took"
        }
    }
}

/// Which screenshots "What FERB saw" shows: one message's (its front window and its tasks' images), one task's (a task
/// this chat didn't send), or every one FERB keeps (Settings → Data).
public enum SeenScope: Equatable {
    case request(String), task(String), all

    /// A conversation turn's: by its message, else by its task; nil when it has neither yet.
    public init?(requestID: String?, taskID: String?) {
        if let requestID, !requestID.isEmpty { self = .request(requestID) }
        else if let taskID, !taskID.isEmpty { self = .task(taskID) }
        else { return nil }
    }

    /// `seen.list`'s parameters.
    public var params: [String: Any] {
        switch self {
        case .request(let id): return ["request_id": id]
        case .task(let id): return ["task_id": id]
        case .all: return [:]
        }
    }

    public var emptyText: String {
        switch self {
        case .all: return "No screenshots kept."
        default: return "FERB kept no screenshot for this."
        }
    }
}

/// A `seen.list` answer: the captures (newest first), how many in all, and how many days FERB keeps them.
public struct SeenList: Equatable {
    public let captures: [SeenCapture]
    public let total: Int
    public let days: Int

    public init(_ reply: [String: Any]) {
        captures = (reply["captures"] as? [[String: Any]] ?? []).compactMap(SeenCapture.init)
        total = max(number(reply["total"]), captures.count)
        days = number(reply["days"]) > 0 ? number(reply["days"]) : 7
    }

    /// Under the list when it doesn't show them all.
    public var moreLine: String? {
        total > captures.count ? "Showing the newest \(captures.count) of \(total)." : nil
    }
}

// MARK: - The one-time clean-up (STORE-402)

/// The last line of a `ferb setup --cleanup` run that is a JSON object (the command prints its plan, or its result, as
/// one JSON line after any progress lines).
public enum FerbJSON {
    public static func last(_ lines: [String]) -> [String: Any]? {
        for line in lines.reversed() {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            guard trimmed.hasPrefix("{"), let data = trimmed.data(using: .utf8),
                  let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { continue }
            return object
        }
        return nil
    }
}

/// `ferb setup --cleanup`'s preview: what the clean-up removes, and the `ok` digest the owner's OK sends back.
public struct CleanupPlan: Equatable {
    public struct Step: Equatable, Identifiable {
        public var id: String { table + "\u{0}" + what }
        public let table: String
        public let rows: Int
        public let what: String
        public let dropped: Bool
    }

    public let steps: [Step]
    public let rows: Int
    public let compact: Bool
    public let bytes: Int
    /// The free disk the compaction needs (about twice what the database holds; older builds: the file's size).
    public let needed: Int
    public let free: Int
    /// The digest the OK carries; nil when there is nothing to do.
    public let ok: String?

    public init?(_ reply: [String: Any]?) {
        guard let reply, reply["steps"] != nil else { return nil }
        steps = (reply["steps"] as? [[String: Any]] ?? []).map {
            Step(table: text($0["table"]), rows: number($0["rows"]), what: text($0["what"]), dropped: flag($0["drop"]))
        }
        rows = number(reply["rows"])
        compact = flag(reply["compact"])
        bytes = number(reply["bytes"])
        needed = reply["needed"] == nil ? bytes : number(reply["needed"])
        free = number(reply["free"])
        ok = (reply["ok"] as? String).flatMap { $0.isEmpty ? nil : $0 }
    }

    /// Nothing to remove and nothing to compact: the clean-up has nothing to do.
    public var nothingToDo: Bool { steps.isEmpty && !compact }

    /// `ferb setup --cleanup --ok DIGEST`.
    public var applyArguments: [String]? { ok.map { ["setup", "--cleanup", "--ok", $0] } }
}

/// `ferb setup --cleanup --ok`'s result line (and its exit status).
public struct CleanupResult: Equatable {
    public let removed: Int
    public let compacted: Bool
    public let before: Int
    public let after: Int
    /// When it didn't compact for want of disk: the free disk it needed and what there was.
    public let needed: Int
    public let free: Int
    public let refused: Bool
    public let stopped: Bool
    public let problem: String?

    public init(_ reply: [String: Any]?, status: Int32) {
        let reply = reply ?? [:]
        removed = (reply["removed"] as? [String: Any] ?? [:]).values.reduce(0) { $0 + number($1) }
        compacted = flag(reply["compacted"])
        before = number(reply["bytes_before"])
        after = number(reply["bytes_after"])
        needed = number(reply["needed"])
        free = number(reply["free"])
        refused = flag(reply["refused"])
        stopped = flag(reply["stopped"])
        let why = text(reply["error"]).isEmpty ? text(reply["not_run"]) : text(reply["error"])
        problem = why.isEmpty ? (status == 0 || refused ? nil : "the clean-up stopped (exit status \(status))") : why
    }

    public var line: String {
        if refused { return "Nothing was removed: what the clean-up would remove changed since you saw it. Look at it again." }
        if let problem { return "The clean-up didn't finish: \(problem). Running it again finishes it." }
        if stopped { return "The clean-up stopped after removing \(removed) rows; running it again finishes it." }
        if compacted { return "Removed \(removed) rows; FERB's database went from \(shownBytes(before)) to \(shownBytes(after))." }
        if needed > 0 {
            return "Removed \(removed) rows. Not compacted: that needs \(shownBytes(needed)) free and the disk has "
                + "\(shownBytes(free)); running the clean-up again compacts it."
        }
        return "Removed \(removed) rows."
    }
}

// MARK: - FERB's own account (APP-1)

/// A browser profile signed into the account the owner made for FERB (`identity.find`).
public struct IdentityCandidate: Identifiable, Equatable {
    public var id: String { browser + "/" + folder }
    public let browser: String
    public let browserName: String
    public let folder: String
    public let name: String
    /// The account the profile is signed in to its browser with (HAND-424); "" when none.
    public let email: String
    /// It holds only FERB's account, as FERB's profile must.
    public let only: Bool
    public let otherAccounts: Int

    public init?(_ row: [String: Any]) {
        guard let browser = row["browser"] as? String, let folder = row["folder"] as? String, !folder.isEmpty else {
            return nil
        }
        self.browser = browser
        self.folder = folder
        browserName = text(row["browser_name"]).isEmpty ? browser : text(row["browser_name"])
        name = text(row["name"]).isEmpty ? folder : text(row["name"])
        email = text(row["email"])
        only = flag(row["only"])
        otherAccounts = number(row["other_accounts"])
    }
}

/// FERB's own account as set now and the profiles found for an address.
public struct IdentityState: Equatable {
    public let address: String
    public let browser: String
    public let profile: String
    /// FERB's profile's name and the account it is signed in to (HAND-424); "" when FERB can't read them.
    public let profileName: String
    public let profileEmail: String
    public let paired: Bool
    /// `browsing.owner_profile`: the owner's profile FERB browses in while it has none of its own.
    public let ownerProfile: String
    public let ownerProfileName: String
    public let ownerProfileEmail: String
    public let candidates: [IdentityCandidate]

    public init(_ reply: [String: Any]) {
        let current = reply["current"] as? [String: Any] ?? [:]
        address = text(current["address"])
        browser = text(current["browser"])
        profile = text(current["profile"])
        profileName = text(current["profile_name"])
        profileEmail = text(current["profile_email"])
        paired = flag(current["paired"])
        ownerProfile = text(current["owner_profile"])
        ownerProfileName = text(current["owner_profile_name"])
        ownerProfileEmail = text(current["owner_profile_email"])
        candidates = (reply["candidates"] as? [[String: Any]] ?? []).compactMap(IdentityCandidate.init)
    }

    public var isSetUp: Bool { !address.isEmpty && !profile.isEmpty }
    /// FERB's profile as shown: its name (its folder when unread) and its account.
    public var profileShown: ProfileName { ProfileName(name: profileName.isEmpty ? profile : profileName, email: profileEmail) }
    /// The one profile that holds only FERB's account, when exactly one does.
    public var usable: [IdentityCandidate] { candidates.filter(\.only) }
}
