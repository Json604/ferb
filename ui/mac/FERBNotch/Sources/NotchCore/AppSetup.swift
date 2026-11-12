import CryptoKit
import Foundation

/// What the packaged FERB.app does at launch or from its menu (L27, L34 C). FERB's background service is a user
/// LaunchAgent (~/Library/LaunchAgents/com.ferb.daemon.plist, DMN-513) that `ferb setup --service` writes and loads, or,
/// for a Developer ID build, FERB.app's own SMAppService registration (DMN-587) with that LaunchAgent as its fallback;
/// the command also enforces where FERB may be installed from. This only reads facts: that LaunchAgent (this app's, an
/// older install's or a checkout's) and its digest, any SMAppService registration, what setup last recorded
/// (~/.ferb/service.json) and whether launchd has FERB running.
public enum AppSetupAction: Equatable {
    case none                               // this app's own build is set up, unchanged and running
    case install                            // nothing set up yet: `ferb setup --service`
    case update                             // another build, an older install, a changed definition or not running
    case confirmReplace(current: String)    // a checkout's LaunchAgent (another installation): ask first
}

/// What launch shows (L34 C, kb area app).
public enum SetupLaunch: Equatable {
    case nothing                 // this app's build runs and setup is finished
    case intro([IntroBeat])      // the first-run intro: every beat, or only those for what is missing
    case quietUpdate             // `ferb setup --service --update`, then the intro only if a grant is missing
    // There is no setup window (APP-685): setup and its errors live in the notch tour only.
}

/// An SMAppService registration's status as macOS reports it (SMAppService.Status): a 2026-11 self-signed build's,
/// which FERB.app unregisters (DMN-513), or a Developer ID build's own (DMN-587).
public enum ServiceStatus: Equatable {
    case notRegistered, enabled, requiresApproval, notFound

    /// macOS still lists the registration, so launchd would start that build's constrained job.
    public var listed: Bool { self == .enabled || self == .requiresApproval }

    /// The registration FERB.app retires (APP-683). SMAppService's status for FERB's bundled plist name also reads
    /// enabled for FERB's own user LaunchAgent: same label, its program inside FERB.app (live 2026-11-04: notFound at
    /// the first launch, enabled at the next, and the unregister that followed had macOS boot FERB's running LaunchAgent
    /// job out). So the status counts only when launchd runs the label from another definition (`loadedFrom`, the
    /// `path =` of `launchctl print`; nil when it names none), or runs nothing under it and no LaunchAgent naming the
    /// label exists. `loaded`: launchd has the label.
    public static func retired(reported: ServiceStatus, loaded: Bool, loadedFrom: String?, launchAgent: String,
                               launchAgentExists: Bool) -> ServiceStatus {
        if loaded { return loadedFrom == launchAgent ? .notRegistered : reported }
        return launchAgentExists ? .notRegistered : reported
    }
}

/// What the setup window says about the background service, in the app's words (plain cause first).
public enum ServicePlan {
    /// The definition's file name inside FERB.app (Contents/Library/LaunchAgents; ferb/device/install.py BUNDLE_SERVICE):
    /// the name a 2026-11 build registered with SMAppService, which FERB.app unregisters.
    public static let plistName = "com.ferb.daemon.plist"
    /// The owner switched FERB off under Login Items → Allow in the Background.
    public static let approval = "macOS needs your OK to run FERB in the background: System Settings → General → " +
        "Login Items → allow FERB."
    /// `ferb setup --service` exits with this when launchd lists FERB's service as switched off
    /// (ferb/device/install.py SWITCHED_OFF_STATUS): the window guides the Login Items visit instead of an error.
    public static let switchedOffStatus: Int32 = 3
}

public enum AppSetupPlan {
    /// `launchAgent`: ~/Library/LaunchAgents/com.ferb.daemon.plist if it names FERB's label, and `launchAgentDigest`
    /// its SHA-256; `registration`: a 2026-11 build's SMAppService registration; `record`: what `ferb setup --service`
    /// last recorded ("build", "app", "definition": the SHA-256 of the LaunchAgent it wrote). Only a checkout's
    /// LaunchAgent asks first; any other (an older build's agent, ~/.ferb/venv's python, a definition someone changed)
    /// and a listed registration are updated quietly (DMN-513, DMN-514). `appService`: this Developer ID build registers
    /// the service itself (DMN-587); once the record says it did ("service": "app"), it is set up while this build's
    /// registration is listed, no LaunchAgent is left and FERB runs. A record from the LaunchAgent (an older build, or
    /// the fallback) is judged as before until the next build tries the registration again. `signerMoving`: the switch
    /// to the Developer ID (APP-904) always sets up.
    public static func decide(launchAgent: [String: Any]?, launchAgentDigest: String?, registration: ServiceStatus,
                              record: [String: Any]?, agentExecutable: String, appPath: String, build: String,
                              daemonRunning: Bool = true, appService: Bool = false,
                              signerMoving: Bool = false) -> AppSetupAction {
        if let arguments = launchAgent?["ProgramArguments"] as? [String], let program = arguments.first,
           program != agentExecutable, isCheckout(arguments) {
            return .confirmReplace(current: program)
        }
        guard let record else { return launchAgent != nil || registration.listed ? .update : .install }
        if signerMoving { return .update }
        if appService && record["service"] as? String == "app" {
            let ours = record["build"] as? String == build && record["app"] as? String == appPath
            return ours && launchAgent == nil && registration.listed && daemonRunning ? .none : .update
        }
        let ours = record["build"] as? String == build && record["app"] as? String == appPath
            && launchAgentDigest != nil && record["definition"] as? String == launchAgentDigest
        return ours && !registration.listed && daemonRunning ? .none : .update
    }

    /// A checkout's LaunchAgent, by its exact shape (ferb/device/install.py daemon_command without a bundle): a venv
    /// python with `-P -m ferb.daemon`. Older installs ran `-m ferb.daemon` without `-P`, or an app's agent.
    public static func isCheckout(_ arguments: [String]) -> Bool {
        arguments.count == 4 && Array(arguments[1...]) == ["-P", "-m", "ferb.daemon"]
            && !arguments[0].hasSuffix(".app/Contents/MacOS/FERB")
    }

    /// The SHA-256 of a definition's bytes, as `ferb setup --service` records it.
    public static func digest(_ data: Data) -> String {
        SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }

    /// The bundled command that installs or updates the background service with no questions; nil when this app's own
    /// build already runs it. `appService`: FERB.app registers the service itself afterwards (DMN-587), so the command
    /// leaves no LaunchAgent and loads nothing.
    public static func serviceArguments(for action: AppSetupAction, appService: Bool = false) -> [String]? {
        let own = appService ? ["--app-service"] : []
        switch action {
        case .none: return nil
        case .install, .confirmReplace: return ["setup", "--service"] + own
        case .update: return ["setup", "--service", "--update"] + own
        }
    }

    /// At launch: the whole intro for a new install or another installation (after the owner's yes), a quiet update
    /// for a replaced build or a stopped service, and otherwise what `needs` still asks for. `tour`: the beats of a
    /// tour a quit interrupted (IntroProgress.interrupted, e.g. macOS's Quit & Reopen during a grant), picked up at its
    /// saved beat (APP-686: never a window).
    public static func atLaunch(_ action: AppSetupAction, needs: IntroNeeds, tour: [IntroBeat]? = nil) -> SetupLaunch {
        switch action {
        case .install, .confirmReplace: return .intro(IntroBeat.allCases)
        case .update: return .quietUpdate
        case .none: return settled(needs, tour: tour)
        }
    }

    /// After a quiet update: as at launch, except that a first run plays the whole tour.
    public static func afterUpdate(_ needs: IntroNeeds, tour: [IntroBeat]? = nil) -> SetupLaunch {
        if let tour, !tour.isEmpty { return .intro(tour) }
        if needs.firstRun { return .intro(IntroBeat.allCases) }
        return settled(needs, tour: nil)
    }

    /// On a finished setup a required grant missing now (switched off since, by the owner or a macOS update) brings
    /// back Allow all and home, and nothing else, at every launch as after an update (APP-686).
    private static func settled(_ needs: IntroNeeds, tour: [IntroBeat]?) -> SetupLaunch {
        if let tour, !tour.isEmpty { return .intro(tour) }
        if needs.setupDone && !needs.firstRun && !needs.missingPermissions.isEmpty { return .intro([.permissions, .home]) }
        let beats = IntroPlan.beats(needs, replay: false)
        return beats.isEmpty ? .nothing : .intro(beats)
    }

    /// Whether this launch is the install's first run (the tour opens with the first-launch clip and the whole tour
    /// plays): a new install or another installation; an install that has neither finished setup nor seen the tour; or
    /// one that hasn't seen this version of the tour yet (`tourSeen` false: an upgrade from an older FERB, APP-635),
    /// in each case unless it is resuming a tour a quit interrupted.
    public static func firstRun(_ action: AppSetupAction, setupDone: Bool, introDone: Bool, resuming: Bool,
                                tourSeen: Bool = true) -> Bool {
        switch action {
        case .install, .confirmReplace: return true
        case .update, .none: return (!setupDone && !introDone || !tourSeen) && !resuming
        }
    }

    /// Replay intro (Settings → General, the menu's Set Up FERB…): every beat. It resets no setup, sign-in or grant.
    public static func replay(_ needs: IntroNeeds) -> SetupLaunch { .intro(IntroPlan.beats(needs, replay: true)) }

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
