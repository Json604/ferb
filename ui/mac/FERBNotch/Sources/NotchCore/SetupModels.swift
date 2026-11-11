import Foundation

/// FERB.app's setup window (L34 C): its steps, in the spec's order.
public enum SetupStep: Int, CaseIterable, Equatable {
    case welcome, ai, permissions, browsers, identity, persona, done
    public var next: SetupStep? { SetupStep(rawValue: rawValue + 1) }
}

/// A macOS permission's state: the daemon's `permissions.Status` values, and FERB.app's own mapped onto them.
public enum GrantStatus: String, Equatable {
    case granted, denied, notDetermined = "not_determined", targetNotRunning = "target_not_running", unknown

    public init(daemon value: Any?) { self = GrantStatus(rawValue: value as? String ?? "") ?? .unknown }

    /// Two grants one row needs (dictation: Microphone and Speech Recognition).
    public static func both(_ a: GrantStatus, _ b: GrantStatus) -> GrantStatus {
        if a == .granted && b == .granted { return .granted }
        if a == .denied || b == .denied { return .denied }
        if a == .notDetermined || b == .notDetermined { return .notDetermined }
        return .unknown
    }
}

/// One Permissions row. `process` is whose grant it is: "daemon" (FERB's agent app, Contents/Helpers/FERB.app, which
/// runs the daemon), or "app" (FERB.app itself).
public struct PermissionRow: Equatable, Identifiable {
    public let id: String
    public let status: GrantStatus
    public let process: String
    public let required: Bool
    public let pane: URL?
    public let requestable: Bool
    public let restartToCheck: Bool
    public let blocked: String
    /// macOS has the switch on, but the running daemon uses it only after a restart (a fresh process read it on,
    /// APP-662); `status` stays what the daemon can use now.
    public let restartNeeded: Bool

    public init(id: String, status: GrantStatus, process: String, required: Bool, pane: URL?, requestable: Bool,
                restartToCheck: Bool = false, blocked: String = "", restartNeeded: Bool = false) {
        self.id = id
        self.status = status
        self.process = process
        self.required = required
        self.pane = pane
        self.requestable = requestable
        self.restartToCheck = restartToCheck
        self.blocked = blocked
        self.restartNeeded = restartNeeded
    }

    public init?(daemon row: [String: Any]) {
        guard let id = row["id"] as? String, !id.isEmpty else { return nil }
        self.init(id: id, status: GrantStatus(daemon: row["status"]), process: row["process"] as? String ?? "daemon",
                  required: row["required"] as? Bool ?? false, pane: (row["pane"] as? String).flatMap(URL.init(string:)),
                  requestable: row["requestable"] as? Bool ?? false,
                  restartToCheck: row["restart_to_check"] as? Bool ?? false, blocked: row["blocked"] as? String ?? "",
                  restartNeeded: row["restart_needed"] as? Bool ?? false)
    }
}

/// What one grant's wait (SetupModel.grantAndConfirm) is doing right now, so the tour's line and the permission's row
/// say it while it happens (APP-665 in the permission beat, APP-679).
public struct GrantWait: Equatable, Sendable {
    public enum Step: Equatable, Sendable {
        /// macOS's own question for this permission is up (a request-style permission).
        case asking
        /// System Settings is opening at the permission's pane.
        case opening
        /// Waiting for the owner to turn FERB on (both FERBs where both were dragged, APP-659).
        case waitingForSwitch
        /// A read of the row from macOS is under way (APP-510; a fresh process's reading, APP-662).
        case checking
        /// FERB's background part restarts so the switch reaches it: `seenOn` when a fresh reading saw the switch on
        /// (APP-662), false for the restart on leaving System Settings that checks it (APP-598).
        case restarting(seenOn: Bool)
        /// FERB's background part stopped answering (macOS's Quit & Reopen can end it) and is being started (APP-598).
        case startingService
    }

    public let id: String
    public private(set) var step: Step
    /// When the wait began.
    public let since: Date
    /// When the current step began.
    public private(set) var stepSince: Date
    /// Reads of the row from macOS so far.
    public private(set) var checks = 0

    public init(id: String, step: Step, at now: Date) {
        self.id = id
        self.step = step
        self.since = now
        self.stepSince = now
    }

    public mutating func move(to next: Step, at now: Date) {
        guard next != step else { return }
        step = next
        stepSince = now
        if next == .checking { checks += 1 }
    }

    /// The start of what the line shows as waited: the owner's part (the switch and each check of it) counts from the
    /// wait's start, so the time never resets every poll; FERB's own steps (asking, opening, a restart) from their own.
    public var shownSince: Date {
        switch step {
        case .waitingForSwitch, .checking: return since
        default: return stepSince
        }
    }
}

public struct AutomationApp: Equatable, Identifiable {
    public var id: String { bundleID }
    public let name: String
    public let bundleID: String
    public let status: GrantStatus

    public init(name: String, bundleID: String, status: GrantStatus) {
        self.name = name
        self.bundleID = bundleID
        self.status = status
    }
}

/// `setup.permissions`'s reply.
public struct PermissionSnapshot: Equatable {
    public var rows: [PermissionRow]
    public let automation: [AutomationApp]
    public let automationPane: URL?
    public let grantTarget: String
    public let appPanes: [String: URL]
    public let setupDone: Bool

    public init?(reply: [String: Any]) {
        guard let rows = reply["rows"] as? [[String: Any]] else { return nil }
        self.rows = rows.compactMap(PermissionRow.init(daemon:))
        automation = (reply["automation"] as? [[String: Any]] ?? []).compactMap { row in
            guard let bundle = row["bundle_id"] as? String, !bundle.isEmpty else { return nil }
            return AutomationApp(name: row["name"] as? String ?? bundle, bundleID: bundle,
                                 status: GrantStatus(daemon: row["status"]))
        }
        automationPane = (reply["automation_pane"] as? String).flatMap(URL.init(string:))
        grantTarget = reply["grant_target"] as? String ?? ""
        appPanes = (reply["app_panes"] as? [String: String] ?? [:]).compactMapValues(URL.init(string:))
        setupDone = reply["setup_done"] as? Bool ?? false
    }
}

/// A browser profile as the Browsers step shows it (`extensions.profile_state`).
public enum ProfileSetupState: String, Equatable {
    case notInstalled = "not_installed", removed, installed, on, paired

    public var text: String {
        switch self {
        case .notInstalled: return "Not added"
        case .removed: return "Removed earlier"
        case .installed: return "Added; turn it on in the browser"
        case .on: return "On · not paired yet"
        case .paired: return "Paired"
        }
    }

    public var canAdd: Bool { self == .notInstalled || self == .removed }
}

public struct BrowserProfileRow: Equatable, Identifiable {
    public var id: String { browser + "/" + folder }
    public let browser: String
    public let folder: String
    public let name: String
    /// The account the profile is signed in to its browser with (HAND-424); "" when none or unreadable.
    public let email: String
    public let state: ProfileSetupState
    public let ferbIdentity: Bool
    public let connected: Bool
    /// The installed extension's version, read afresh from the profile's files; "" when unknown (APP-883).
    public let version: String
    /// Below the version FERB needs: its pairing would be refused until it updates.
    public let tooOld: Bool
    /// Why a paired profile isn't connected, in the daemon's words (refused, or too old as last reported); "" none.
    public let problem: String
    /// APP-892: paired, but the copy of FERB's extension that answers there now is a new one (added again: a new
    /// identity), connected and not paired: opening the profile won't connect it, pairing it again will.
    public let needsPairing: Bool

    public init(browser: String, folder: String, name: String, email: String = "", state: ProfileSetupState,
                ferbIdentity: Bool, connected: Bool, version: String = "", tooOld: Bool = false, problem: String = "",
                needsPairing: Bool = false) {
        self.browser = browser
        self.folder = folder
        self.name = name
        self.email = email
        self.state = state
        self.ferbIdentity = ferbIdentity
        self.connected = connected
        self.version = version
        self.tooOld = tooOld
        self.problem = problem
        self.needsPairing = needsPairing
    }

    /// The row's state as it stands now (APP-883): its problem first, then too old, then paired and whether its
    /// browser is connected right now, else the setup state.
    public var stateText: String {
        if !problem.isEmpty { return "Not connected: " + problem }
        if tooOld { return "Too old (version \(version)): update FERB's extension in this profile" }
        if needsPairing { return "Needs pairing again: FERB's extension here is a new copy" }
        if state == .paired { return connected ? "Paired · connected" : "Paired · not connected (open this profile)" }
        return state.text
    }

    /// A row that needs the owner: a problem, an extension too old, or a new copy to pair (APP-892).
    public var needsAttention: Bool { !problem.isEmpty || tooOld || needsPairing }

    /// The row offers Pair: FERB's extension is on there and not paired, or a new copy needs pairing again (APP-892).
    public var offersPair: Bool { !ferbIdentity && (state == .on || needsPairing) }
}

public struct BrowserRow: Equatable, Identifiable {
    public var id: String { browser }
    public let browser: String
    public let label: String
    public let requested: Bool
    public let manual: Bool
    public let unconfirmed: String
    public let profiles: [BrowserProfileRow]
}

/// An installed Chromium-family browser FERB doesn't support yet (HAND-415): its name and the daemon's plain line.
public struct UnsupportedBrowser: Equatable, Identifiable {
    public var id: String { bundleID }
    public let app: String
    public let bundleID: String
    public let message: String
}

/// `setup.browsers`'s reply. `problem`: what kept FERB from connecting the browsers (no Full Disk Access yet, or a
/// browser folder that refused FERB's files, DMN-507), in the daemon's words; empty when nothing did. `browsers` holds
/// only browsers whose app is installed (HAND-414); `unsupported`, the Chromium-family ones FERB can't pair yet.
public struct BrowserSnapshot: Equatable {
    public let storeURL: URL?
    public let extensionDir: String
    public let browsers: [BrowserRow]
    public let unsupported: [UnsupportedBrowser]
    public let problem: String

    public init?(reply: [String: Any]) {
        guard let browsers = reply["browsers"] as? [[String: Any]] else { return nil }
        storeURL = (reply["store_url"] as? String).flatMap(URL.init(string:))
        extensionDir = reply["extension_dir"] as? String ?? ""
        problem = reply["problem"] as? String ?? ""
        unsupported = (reply["unsupported"] as? [[String: Any]] ?? []).compactMap { row in
            guard let app = row["app"] as? String, !app.isEmpty else { return nil }
            return UnsupportedBrowser(app: app, bundleID: row["bundle_id"] as? String ?? app,
                                      message: row["message"] as? String ?? "\(app) isn't supported by FERB yet")
        }
        self.browsers = browsers.compactMap { browser in
            guard let name = browser["browser"] as? String, !name.isEmpty else { return nil }
            let profiles = (browser["profiles"] as? [[String: Any]] ?? []).compactMap { p -> BrowserProfileRow? in
                guard let folder = p["folder"] as? String, !folder.isEmpty,
                      let state = ProfileSetupState(rawValue: p["state"] as? String ?? "") else { return nil }
                return BrowserProfileRow(browser: name, folder: folder, name: p["name"] as? String ?? folder,
                                         email: p["email"] as? String ?? "", state: state, ferbIdentity: p["ferb_identity"] as? Bool ?? false,
                                         connected: p["connected"] as? Bool ?? false, version: p["version"] as? String ?? "",
                                         tooOld: p["too_old"] as? Bool ?? false, problem: p["problem"] as? String ?? "",
                                         needsPairing: p["needs_pairing"] as? Bool ?? false)
            }
            return BrowserRow(browser: name, label: browser["label"] as? String ?? name,
                              requested: browser["requested"] as? Bool ?? false,
                              manual: browser["manual"] as? Bool ?? false,
                              unconfirmed: browser["unconfirmed"] as? String ?? "", profiles: profiles)
        }
    }
}

/// A sign-in the daemon runs for a runtime (`runtimes.sign_in`): idle, waiting (for the browser or a pasted code),
/// signed_in, or failed (`why` gives the exact steps to sign in by hand).
public struct SignInStatus: Equatable {
    public let state: String
    public let url: URL?
    public let why: String

    public init?(_ value: Any?) {
        guard let row = value as? [String: Any], let state = row["state"] as? String else { return nil }
        self.state = state
        url = (row["url"] as? String).flatMap { $0.isEmpty ? nil : URL(string: $0) }
        why = row["why"] as? String ?? ""
    }

    public var waiting: Bool { state == "waiting" }
}

/// The last call a runtime refused for its account (APP-882): signed out or a usage limit, when, and the owner's step.
/// It stays until a later call the runtime answers, whatever the sign-in's presence check says.
public struct RuntimeRefusal: Equatable {
    public let account: String
    public let at: Date?
    public let fix: String

    public init?(_ value: Any?) {
        guard let row = value as? [String: Any], let account = row["account"] as? String, !account.isEmpty else {
            return nil
        }
        self.account = account
        at = (row["at"] as? String).flatMap(SetupDates.parse)
        fix = row["fix"] as? String ?? ""
    }

    /// "Last call refused: signed out at 14:02. Sign in to Claude again in Settings → AI."
    public func text(timeZone: TimeZone = .current) -> String {
        let what = account == "signed_out" ? "signed out" : account == "usage_limit" ? "usage limit reached" : account
        let when = at.map { " at " + SetupDates.clock($0, timeZone: timeZone) } ?? ""
        let step = fix.isEmpty ? "" : " " + fix.prefix(1).uppercased() + fix.dropFirst() + (fix.hasSuffix(".") ? "" : ".")
        return "Last call refused: \(what)\(when).\(step)"
    }
}

/// Times the AI page shows: the daemon's ISO times, and a clock time for a line.
public enum SetupDates {
    /// Python's isoformat (microseconds and an offset): the fraction is dropped, a line needs only the minute.
    public static func parse(_ text: String) -> Date? {
        ISO8601DateFormatter().date(from: text.replacingOccurrences(of: #"\.\d+"#, with: "", options: .regularExpression))
    }

    public static func clock(_ date: Date, timeZone: TimeZone = .current) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = timeZone
        formatter.dateFormat = Calendar.current.isDateInToday(date) ? "HH:mm" : "d MMM, HH:mm"
        return formatter.string(from: date)
    }
}

public struct RuntimeRow: Equatable, Identifiable {
    public var id: String { name }
    public let name: String
    public let usable: Bool
    public let fix: String
    public let model: String
    public let signedIn: Bool
    public let signIn: SignInStatus?
    /// The connection's own effort level ("" for the model's default), APP-881.
    public var effort: String = ""
    public var refused: RuntimeRefusal? = nil
}

/// What one role runs on now (APP-882): its connection, model and effort ("" for the default).
public struct RoleAI: Equatable {
    public let runtime: String
    public let model: String
    public let effort: String
}

/// `runtimes.status`'s reply.
public struct AISnapshot: Equatable {
    public let roles: [String: String]
    public let runtimes: [RuntimeRow]
    /// role -> its connection, model and effort (an older daemon gives none)
    public let roleDetails: [String: RoleAI]
    public static let roleOrder = ["orchestrator", "worker", "checker"]

    public init?(reply: [String: Any]) {
        guard let roles = reply["roles"] as? [String: String], let rows = reply["runtimes"] as? [[String: Any]] else {
            return nil
        }
        self.roles = roles
        runtimes = rows.compactMap { row in
            guard let name = row["name"] as? String else { return nil }
            return RuntimeRow(name: name, usable: row["usable"] as? Bool ?? false, fix: row["fix"] as? String ?? "",
                              model: row["model"] as? String ?? "", signedIn: row["signed_in"] as? Bool ?? false,
                              signIn: SignInStatus(row["sign_in"]), effort: row["effort"] as? String ?? "",
                              refused: RuntimeRefusal(row["refused"]))
        }
        var details: [String: RoleAI] = [:]
        for (role, value) in reply["role_details"] as? [String: Any] ?? [:] {
            guard let row = value as? [String: Any], let runtime = row["runtime"] as? String else { continue }
            details[role] = RoleAI(runtime: runtime, model: row["model"] as? String ?? "", effort: row["effort"] as? String ?? "")
        }
        roleDetails = details
    }

    /// The runtime every role uses, when they agree.
    public var current: String? {
        let used = Set(roles.values)
        return used.count == 1 ? used.first : nil
    }

    /// The connections the roles run on, each once, in the roles' order (planner, worker, checker).
    public var runtimesInUse: [String] {
        var seen: [String] = []
        for role in Self.roleOrder { if let name = roles[role], !seen.contains(name) { seen.append(name) } }
        return seen
    }

    /// The roles that run on a connection now, in order, in the owner's words: "Planner · Worker · Checker" (APP-882).
    public func rolesText(_ name: String) -> String {
        Self.roleOrder.filter { roles[$0] == name }.map(RoleCopy.label).joined(separator: " · ")
    }

    public func row(_ name: String) -> RuntimeRow? { runtimes.first { $0.name == name } }
    public func usable(_ name: String) -> Bool { runtimes.first { $0.name == name }?.usable ?? false }
    /// Whether FERB uses this runtime now: a role runs on it (the daemon's `roles`), whatever else is signed in (APP-655).
    public func inUse(_ name: String) -> Bool { roles.values.contains(name) }
    /// Whether `role` (nil: any role) runs on `name` now.
    public func runs(_ name: String, role: String?) -> Bool { role.map { roles[$0] == name } ?? inUse(name) }
    /// The API key FERB uses now, when a role runs on one.
    public var apiKeyInUse: APIProvider? { APIProvider.allCases.first { inUse($0.rawValue) } }
    public func signedIn(_ name: String) -> Bool { runtimes.first { $0.name == name }?.signedIn ?? false }
    /// The API keys the Keychain holds (presence only): each gets its own card (APP-792).
    public var savedKeys: [APIProvider] { APIProvider.allCases.filter { signedIn($0.rawValue) } }
    /// The providers with no key saved: the paste box offers these.
    public var unsavedKeys: [APIProvider] { APIProvider.allCases.filter { !signedIn($0.rawValue) } }
    public func signIn(_ name: String) -> SignInStatus? { runtimes.first { $0.name == name }?.signIn }

    public static func label(_ runtime: String) -> String {
        ["codex": "ChatGPT (Codex)", "claude": "Claude", "anthropic": "your Anthropic API key",
         "openai": "your OpenAI API key", "google": "your Google API key"][runtime] ?? runtime
    }
}

/// `runtimes.models`' reply (RUN-429): the models one connection can use (an API key's live list from its provider, a
/// sign-in's from its runtime), the model it runs now, and each role's own model. The owner may also type any model id.
public struct ModelList: Equatable {
    public let runtime: String
    public let current: String
    public let models: [String]
    /// The provider's words when it gave no list (the owner can still type a model).
    public let problem: String?
    /// role -> model chosen for that role on this connection
    public let roles: [String: String]
    /// The connection's own effort ("" for the model's default) and each role's own on it (APP-881).
    public let effort: String
    public let roleEfforts: [String: String]
    /// model -> the effort levels its provider documents (none listed: only Default and a typed level)
    public let levels: [String: [String]]
    /// model -> its default level where documented
    public let defaultEfforts: [String: String]

    public init?(reply: [String: Any]) {
        guard let runtime = reply["runtime"] as? String else { return nil }
        self.runtime = runtime
        current = reply["model"] as? String ?? ""
        problem = (reply["problem"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        effort = reply["effort"] as? String ?? ""
        var roles: [String: String] = [:]
        var efforts: [String: String] = [:]
        for (role, value) in reply["roles"] as? [String: Any] ?? [:] {
            guard let row = value as? [String: Any], row["runtime"] as? String == runtime else { continue }
            if let model = row["model"] as? String, !model.isEmpty { roles[role] = model }
            if let level = row["effort"] as? String, !level.isEmpty { efforts[role] = level }
        }
        self.roles = roles
        roleEfforts = efforts
        levels = (reply["levels"] as? [String: [String]]) ?? [:]
        defaultEfforts = (reply["default_efforts"] as? [String: String]) ?? [:]
        let listed = (reply["models"] as? [String] ?? []).filter { !$0.isEmpty }
        // the model in use always shows, even when the list doesn't have it (one the owner typed)
        models = current.isEmpty || listed.contains(current) ? listed : [current] + listed
    }

    /// The effort set for a role on this connection (nil: the connection's own); "" when none.
    public func effort(for role: String?) -> String { role.map { roleEfforts[$0] ?? "" } ?? effort }

    /// The levels the picker offers for `model` ("" : the connection's model); a level set but not listed shows too.
    public func levels(for model: String, keeping set: String = "") -> [String] {
        let listed = levels[model.isEmpty ? current : model] ?? []
        return set.isEmpty || listed.contains(set) ? listed : listed + [set]
    }

    /// The picker's Default entry: "Default (medium)", or "Default (the model's own)" when not documented.
    public func defaultLabel(for model: String) -> String {
        let level = defaultEfforts[model.isEmpty ? current : model] ?? ""
        return level.isEmpty ? "Default (the model's own)" : "Default (\(level))"
    }

    /// What the picker shows for a model: the list's own entry, else "Other…" (typed).
    public func listed(_ model: String) -> Bool { models.contains(model) }

    /// The model a role runs on this connection: its own, else "" (the connection's) (APP-793); nil: the connection's.
    public func current(for role: String?) -> String { role.map { roles[$0] ?? "" } ?? current }

    /// The picker's models for a role: the connection's list, with a role's own typed model in it too.
    public func options(for role: String?) -> [String] {
        let mine = current(for: role)
        return mine.isEmpty || models.contains(mine) ? models : [mine] + models
    }
}

/// The line after a model or effort is saved (APP-882): "FERB uses …" only when a role runs on that connection (for a
/// role's own: when that role does); otherwise it says it is saved for when one does.
public enum ModelSaveLine {
    public static func saved(model: String, effort: String, connectionModel: String, runtime: String, role: String?,
                             running: Bool) -> String {
        let name = AISnapshot.label(runtime)
        let shown = model.isEmpty ? connectionModel : model
        let what = shown + (effort.isEmpty ? "" : " at \(effort) effort")
        guard let role else {
            return running ? "FERB uses \(what) now." : "Saved for \(name); FERB uses it when a role runs on \(name)."
        }
        let who = RoleCopy.label(role).lowercased()
        return running ? "The \(who) uses \(what) now."
            : "Saved for the \(who) on \(name); it's used when the \(who) runs on \(name)."
    }
}

/// `runtimes.check_calls`' reply (T-1069): one line per call shape FERB really makes, and the total cost.
public struct ConnectionCheck: Equatable {
    public struct Line: Equatable, Identifiable {
        public var id: String { name }
        public let name: String
        public let ok: Bool
        public let detail: String
        public let model: String
        public let typed: String
        public let seconds: Double
        public let tokens: Int
        public let cost: Double?
        /// The call it ran inside (the tool-call lines: the worker call's time and tokens are on its line), RUN-439
        public var partOf: String = ""

        /// "✓ Planner call — claude-opus-5-5, typed strict, 2.1 s, 1,204 tokens, $0.0031" (the provider's words after a ✗);
        /// a tool-call line: "✓ Tool call with no arguments — ran inside the worker call"
        public var text: String {
            let mark = ok ? "✓ " : "✗ "
            let fault = ok || detail.isEmpty ? "" : ": " + detail
            if !partOf.isEmpty { return mark + name + " — ran inside the " + partOf + fault }
            var facts = [model, "typed \(typed)", String(format: "%.1f s", seconds), "\(tokens) tokens"].filter { !$0.isEmpty }
            if let cost { facts.append(String(format: "$%.4f", cost)) }
            return mark + name + " — " + facts.joined(separator: ", ") + fault
        }
    }
    public let lines: [Line]
    public let ok: Bool
    public let totalCost: Double?

    public init(reply: [String: Any]) {
        lines = (reply["lines"] as? [[String: Any]] ?? []).map { row in
            Line(name: row["name"] as? String ?? "", ok: row["ok"] as? Bool ?? false, detail: row["detail"] as? String ?? "",
                 model: row["model"] as? String ?? "", typed: row["typed"] as? String ?? "none",
                 seconds: row["seconds"] as? Double ?? Double(row["seconds"] as? Int ?? 0),
                 tokens: (row["input_tokens"] as? Int ?? 0) + (row["output_tokens"] as? Int ?? 0),
                 cost: row["cost_usd"] as? Double, partOf: row["part_of"] as? String ?? "")
        }
        ok = reply["ok"] as? Bool ?? false
        totalCost = reply["total_cost_usd"] as? Double
        runtime = reply["runtime"] as? String ?? ""
        limit = (reply["lines"] as? [[String: Any]])?.first?["limit_s"] as? Double
    }

    /// The connection this check ran on (RUN-439: shown only under it), and each call's limit in seconds.
    public let runtime: String
    public let limit: Double?

    public var summary: String {
        let passed = lines.filter(\.ok).count
        let cost = totalCost.map { String(format: " Cost: $%.4f.", $0) } ?? ""
        let limit = limit.map { String(format: " Each call is given up after %.0f s.", $0) } ?? ""
        return "\(passed) of \(lines.count) checks passed.\(cost)\(limit)"
    }
}

/// The owner's words for FERB's roles.
public enum RoleCopy {
    public static func label(_ role: String) -> String {
        ["orchestrator": "Planner", "worker": "Worker", "checker": "Checker"][role] ?? role
    }
}

public enum APIProvider: String, CaseIterable, Identifiable {
    case anthropic, openai, google
    public var id: String { rawValue }
    public var label: String {
        switch self {
        case .anthropic: return "Anthropic"
        case .openai: return "OpenAI"
        case .google: return "Google"
        }
    }
}

/// A key FERB.app hands the daemon (GATE-543), which stores it through FERB's signed secret helper.
public enum APIKeyEntry {
    /// A key the Keychain line carries as one word: printable ASCII with no spaces, quotes or backslashes.
    public static func isStorable(_ key: String) -> Bool {
        !key.isEmpty && key.unicodeScalars.allSatisfy { scalar in
            scalar.value > 32 && scalar.value < 127 && scalar != "\"" && scalar != "'" && scalar != "\\"
        }
    }

}

/// Getting to know you (`setup.persona`, MEM-13): what FERB saw on this Mac, shown once as
/// "here's what I saw"; nothing to answer (MEM-12).
public enum PersonaState: Equatable {
    case loading                 // not asked for yet: the window calls `setup.persona {}` to start the scan
    case scanning
    case seen(summary: String)   // what FERB saw and saved as observed memory
    case finished                // closed or unavailable: nothing to show
    case failed(String)

    public init(reply: [String: Any]) {
        switch reply["state"] as? String {
        case "scanning": self = .scanning
        case "not_started": self = .loading
        case "seen": self = .seen(summary: reply["summary"] as? String ?? "")
        case "failed": self = .failed(reply["error"] as? String ?? "The app-use scan failed.")
        default: self = .finished
        }
    }
}

/// The owner's words for each row.
public enum PermissionCopy {
    public static func label(_ id: String) -> String {
        ["accessibility": "Accessibility", "screen_recording": "Screen Recording", "full_disk_access": "Full Disk Access",
         "input_monitoring": "Input Monitoring", "clipboard": "Paste from Other Apps",
         "notifications": "Notifications", "microphone": "Microphone"][id] ?? id
    }

    public static func reason(_ id: String) -> String {
        ["accessibility": "FERB clicks and types in apps for you.",
         "screen_recording": "FERB sees the screen to check its work.",
         "full_disk_access": "FERB reads and tidies your files.",
         "input_monitoring": "FERB notices when you're typing, so it stays out of your way.",
         "clipboard": "FERB keeps what you copy on this Mac for 7 days. After macOS asks, set FERB to Allow.",
         "notifications": "FERB tells you when it needs you or finishes.",
         "microphone": "FERB turns your voice, and the sound of an app it listens to during a task, into text on this Mac (Microphone and Speech Recognition)."][id] ?? ""
    }

    public static func status(_ status: GrantStatus) -> String {
        switch status {
        case .granted: return "On"
        case .denied: return "Off"
        case .notDetermined: return "Not asked yet"
        case .targetNotRunning: return "App not running"
        case .unknown: return "Unknown"
        }
    }
}

/// A command's output as whole lines (the setup window shows the latest one).
public struct LineSplitter {
    private var partial = ""

    public init() {}

    public mutating func append(_ text: String) -> [String] {
        partial += text
        var parts = partial.components(separatedBy: "\n")
        partial = parts.removeLast()
        return parts.map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
    }

    public mutating func finish() -> String? {
        let rest = partial.trimmingCharacters(in: .whitespaces)
        partial = ""
        return rest.isEmpty ? nil : rest
    }
}

/// Restarting FERB's background service (`com.ferb.daemon`): kickstart it when launchd has it; load it when it is
/// installed but not loaded.
public enum LaunchctlPlan {
    public static let label = "com.ferb.daemon"

    public static func restart(uid: uid_t, plist: String, plistExists: Bool) -> [[String]] {
        var commands = [["kickstart", "-k", "gui/\(uid)/\(label)"]]
        if plistExists { commands.append(["bootstrap", "gui/\(uid)", plist]) }
        return commands
    }

    /// Starts the job only if it isn't running (no -k: a running daemon is never killed).
    public static func start(uid: uid_t) -> [String] { ["kickstart", "gui/\(uid)/\(label)"] }

    /// The definition launchd loaded FERB's job from: the first `path = ` line of `launchctl print gui/<uid>/<label>`
    /// (ferb/device/install.py `_loaded_from` reads the same line); nil when it names none.
    public static func loadedPath(_ output: String) -> String? {
        for line in output.split(separator: "\n") {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("path = ") { return String(trimmed.dropFirst("path = ".count)) }
        }
        return nil
    }

    /// From `launchctl print-disabled gui/<uid>`: FERB's service is switched off (Login Items → Allow in the
    /// Background). Older macOS prints `=> true`, newer `=> disabled` (ferb/device/install.py Installer.disabled).
    public static func disabled(_ output: String) -> Bool {
        for line in output.split(separator: "\n") {
            let parts = line.components(separatedBy: "=>")
            guard parts.count == 2 else { continue }
            let name = parts[0].trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "\""))
            if name == label {
                let state = parts[1].trimmingCharacters(in: .whitespaces)
                return state == "disabled" || state == "true"
            }
        }
        return false
    }
}
