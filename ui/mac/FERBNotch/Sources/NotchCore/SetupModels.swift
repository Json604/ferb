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

    public init(id: String, status: GrantStatus, process: String, required: Bool, pane: URL?, requestable: Bool,
                restartToCheck: Bool = false, blocked: String = "") {
        self.id = id
        self.status = status
        self.process = process
        self.required = required
        self.pane = pane
        self.requestable = requestable
        self.restartToCheck = restartToCheck
        self.blocked = blocked
    }

    public init?(daemon row: [String: Any]) {
        guard let id = row["id"] as? String, !id.isEmpty else { return nil }
        self.init(id: id, status: GrantStatus(daemon: row["status"]), process: row["process"] as? String ?? "daemon",
                  required: row["required"] as? Bool ?? false, pane: (row["pane"] as? String).flatMap(URL.init(string:)),
                  requestable: row["requestable"] as? Bool ?? false,
                  restartToCheck: row["restart_to_check"] as? Bool ?? false, blocked: row["blocked"] as? String ?? "")
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
    public let state: ProfileSetupState
    public let ferbIdentity: Bool
    public let connected: Bool
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

/// `setup.browsers`'s reply. `problem`: what kept FERB from connecting the browsers (no Full Disk Access yet, or a
/// browser folder that refused FERB's files, DMN-507), in the daemon's words; empty when nothing did.
public struct BrowserSnapshot: Equatable {
    public let storeURL: URL?
    public let extensionDir: String
    public let browsers: [BrowserRow]
    public let problem: String

    public init?(reply: [String: Any]) {
        guard let browsers = reply["browsers"] as? [[String: Any]] else { return nil }
        storeURL = (reply["store_url"] as? String).flatMap(URL.init(string:))
        extensionDir = reply["extension_dir"] as? String ?? ""
        problem = reply["problem"] as? String ?? ""
        self.browsers = browsers.compactMap { browser in
            guard let name = browser["browser"] as? String, !name.isEmpty else { return nil }
            let profiles = (browser["profiles"] as? [[String: Any]] ?? []).compactMap { p -> BrowserProfileRow? in
                guard let folder = p["folder"] as? String, !folder.isEmpty,
                      let state = ProfileSetupState(rawValue: p["state"] as? String ?? "") else { return nil }
                return BrowserProfileRow(browser: name, folder: folder, name: p["name"] as? String ?? folder,
                                         state: state, ferbIdentity: p["ferb_identity"] as? Bool ?? false,
                                         connected: p["connected"] as? Bool ?? false)
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

public struct RuntimeRow: Equatable, Identifiable {
    public var id: String { name }
    public let name: String
    public let usable: Bool
    public let fix: String
    public let model: String
    public let signedIn: Bool
    public let signIn: SignInStatus?
}

/// `runtimes.status`'s reply.
public struct AISnapshot: Equatable {
    public let roles: [String: String]
    public let runtimes: [RuntimeRow]

    public init?(reply: [String: Any]) {
        guard let roles = reply["roles"] as? [String: String], let rows = reply["runtimes"] as? [[String: Any]] else {
            return nil
        }
        self.roles = roles
        runtimes = rows.compactMap { row in
            guard let name = row["name"] as? String else { return nil }
            return RuntimeRow(name: name, usable: row["usable"] as? Bool ?? false, fix: row["fix"] as? String ?? "",
                              model: row["model"] as? String ?? "", signedIn: row["signed_in"] as? Bool ?? false,
                              signIn: SignInStatus(row["sign_in"]))
        }
    }

    /// The runtime every role uses, when they agree.
    public var current: String? {
        let used = Set(roles.values)
        return used.count == 1 ? used.first : nil
    }

    public func usable(_ name: String) -> Bool { runtimes.first { $0.name == name }?.usable ?? false }
    public func signedIn(_ name: String) -> Bool { runtimes.first { $0.name == name }?.signedIn ?? false }
    public func signIn(_ name: String) -> SignInStatus? { runtimes.first { $0.name == name }?.signIn }

    public static func label(_ runtime: String) -> String {
        ["codex": "ChatGPT (Codex)", "claude": "Claude", "anthropic": "your Anthropic API key",
         "openai": "your OpenAI API key", "google": "your Google API key"][runtime] ?? runtime
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

/// The Keychain item the daemon's API runtimes read (service "ferb", account "api/<provider>"), written by FERB.app on
/// `security -i`'s stdin (rule 14: never an argument).
public enum APIKeyEntry {
    public static func account(_ provider: APIProvider) -> String { "api/" + provider.rawValue }

    /// A key the Keychain line carries as one word: printable ASCII with no spaces, quotes or backslashes.
    public static func isStorable(_ key: String) -> Bool {
        !key.isEmpty && key.unicodeScalars.allSatisfy { scalar in
            scalar.value > 32 && scalar.value < 127 && scalar != "\"" && scalar != "'" && scalar != "\\"
        }
    }

    /// `security -i`'s line that stores it (`-U` replaces an earlier key for that provider), or nil.
    public static func command(provider: APIProvider, key: String) -> String? {
        isStorable(key) ? "add-generic-password -U -s ferb -a \(account(provider)) -w \(key)\n" : nil
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
