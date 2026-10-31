import Foundation

/// What the packaged FERB.app does at launch or from its menu (L27, L34 C). `ferb setup --service` does the file work
/// and enforces where FERB may be installed from; the app then registers the background service whose definition is
/// inside the signed app (SMAppService, notes/daemon.md decision 2). This only reads facts: an old user LaunchAgent
/// (~/Library/LaunchAgents/com.ferb.daemon.plist, from before the app registered its own, or a checkout's), the app
/// service's status, what setup last recorded (~/.ferb/service.json) and whether launchd has FERB running.
public enum AppSetupAction: Equatable {
    case none                               // this app's own build is set up and running
    case install                            // nothing set up yet: `ferb setup --service`
    case update                             // this app replaced by another build, not running, or an old agent to retire
    case confirmReplace(current: String)    // another installation (a checkout, or a FERB.app elsewhere): ask first
}

/// What launch shows (L34 C).
public enum SetupLaunch: Equatable {
    case nothing                 // this app's build runs and setup is finished
    case window(SetupStep)       // the setup window at that step
    case quietUpdate             // `ferb setup --service --update`, no window unless it fails or a grant is missing
}

/// The background service's status as macOS reports it (SMAppService.Status).
public enum ServiceStatus: Equatable {
    case notRegistered, enabled, requiresApproval, notFound
}

/// What the setup window says about the background service, in the app's words (plain cause first).
public enum ServicePlan {
    /// The definition's file name inside FERB.app (Contents/Library/LaunchAgents; ferb/device/install.py BUNDLE_SERVICE).
    public static let plistName = "com.ferb.daemon.plist"
    public static let approval = "macOS needs your OK to run FERB in the background: System Settings → General → " +
        "Login Items → allow FERB."
    public static let notFound = "This copy of FERB.app has no background service inside it. Download FERB again."
    public static let notStarted = "FERB's background service didn't start. Try again; if it keeps failing, " +
        "export a diagnostic file from Settings → Advanced."

    /// Why registering failed: the reason macOS gave, plain.
    public static func failed(_ reason: String) -> String {
        "macOS didn't turn on FERB's background service (\(reason)). Try again."
    }

    /// After `register()`: nil when the service is on, else what the owner reads.
    public static func problem(_ status: ServiceStatus) -> String? {
        switch status {
        case .enabled: return nil
        case .requiresApproval: return approval
        case .notFound: return notFound
        case .notRegistered: return failed("it stayed off")
        }
    }
}

public enum AppSetupPlan {
    /// `legacyAgent`: the old user LaunchAgent (com.ferb.daemon) if one is there; `record`: what `ferb setup --service`
    /// last recorded ("build", "app"). The old agent always goes (the app's service replaces it): quietly when it runs
    /// this very app, after the owner's yes when it runs another installation.
    public static func decide(legacyAgent: [String: Any]?, service: ServiceStatus, record: [String: Any]?,
                              agentExecutable: String, appPath: String, build: String,
                              daemonRunning: Bool = true) -> AppSetupAction {
        if let arguments = legacyAgent?["ProgramArguments"] as? [String], let program = arguments.first {
            return program == agentExecutable ? .update : .confirmReplace(current: program)
        }
        guard let record else { return service == .enabled ? .update : .install }
        let ours = record["build"] as? String == build && record["app"] as? String == appPath
        return ours && service == .enabled && daemonRunning ? .none : .update
    }

    /// The bundled command that installs or updates the background service with no questions; nil when this app's own
    /// build already runs it.
    public static func serviceArguments(for action: AppSetupAction) -> [String]? {
        switch action {
        case .none: return nil
        case .install, .confirmReplace: return ["setup", "--service"]
        case .update: return ["setup", "--service", "--update"]
        }
    }

    /// At launch: the setup window for a new install or another installation (after the owner's yes), a quiet update for
    /// a replaced build or a stopped service, and otherwise the window only while setup isn't finished. `resume`: the
    /// step a Grant left when macOS restarted FERB (Screen Recording's "Quit & Reopen"), reopened done or not.
    public static func atLaunch(_ action: AppSetupAction, setupDone: Bool, resume: SetupStep? = nil) -> SetupLaunch {
        switch action {
        case .install, .confirmReplace: return .window(.welcome)
        case .update: return .quietUpdate
        case .none:
            if let resume { return .window(resume) }
            return setupDone ? .nothing : .window(.welcome)
        }
    }

    /// FERB's own Codex sign-in (C2): Choose your AI's Sign in with ChatGPT runs it.
    /// FERB never uses the owner's personal Codex login.
    public static let codexLoginArguments = ["setup", "--codex-login"]

    public static func needsConfirmation(_ action: AppSetupAction) -> Bool {
        if case .confirmReplace = action { return true }
        return false
    }
}

/// Turns a process's output into text as it arrives. A character split across two reads waits for its other
/// bytes; prompts that end without a newline appear at once.
public struct ConsoleDecoder {
    private var pending: [UInt8] = []

    public init() {}

    public mutating func append(_ data: Data) -> String {
        pending += data
        var cut = pending.count
        var index = pending.count - 1
        var continuation = 0
        while index >= 0 && continuation < 3 {
            let byte = pending[index]
            if byte & 0xC0 == 0x80 { index -= 1; continuation += 1; continue }
            let length = byte & 0xE0 == 0xC0 ? 2 : byte & 0xF0 == 0xE0 ? 3 : byte & 0xF8 == 0xF0 ? 4 : 1
            if length > pending.count - index { cut = index }
            break
        }
        let text = String(decoding: pending[..<cut], as: UTF8.self)
        pending = Array(pending[cut...])
        return text
    }
}
