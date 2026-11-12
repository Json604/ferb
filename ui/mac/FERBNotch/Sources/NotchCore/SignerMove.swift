import Foundation

/// Who signed FERB.app (T-27), in the words the setup record keeps (ferb/device/install.py `Installer.signer`):
/// "team:<TEAMID>" for an Apple Developer ID, "leaf:<SHA-1>" for FERB's self-made release certificate, else "adhoc".
public enum Signer {
    public static func words(team: String?, leafSHA1: String?) -> String {
        if let team, !team.isEmpty, team != "not set" { return "team:\(team)" }
        if let leafSHA1, !leafSHA1.isEmpty { return "leaf:\(leafSHA1.lowercased())" }
        return "adhoc"
    }

    /// A Developer ID: macOS ties FERB's registration, login item and grants to the team, so they hold across builds.
    public static func isTeam(_ signer: String) -> Bool { signer.hasPrefix("team:") }
}

/// The switch to the Developer ID (T-27, APP-904): the first launch of a team-signed FERB.app over an install that
/// setup ran under another signer. Only then does FERB.app move its background service into the app (DMN-587), move
/// its login item, clear macOS's permission entries for its own app IDs once (APP-902) and say why macOS asks for every
/// permission again. On a self-signed or ad-hoc build, and on a new install, nothing moves.
public enum SignerMove {
    /// FERB.app's defaults: the signer a move was started for, until the service runs under it (a quit mid-move resumes
    /// it at the next launch, after the record already names the new signer).
    public static let pendingKey = "signerMove.pending"
    /// The signer FERB.app last cleared its permission entries for: once per switch, never at a later launch.
    public static let permissionsResetKey = "signerMove.permissionsReset"

    /// `record`: what `ferb setup --service` last recorded; its `signer` is missing in a record written before T-27,
    /// which only a self-signed or ad-hoc build wrote. `own`: this app's own signer.
    public static func starts(record: [String: Any]?, own: String) -> Bool {
        guard Signer.isTeam(own), let record else { return false }
        return record["signer"] as? String != own
    }

    /// The macOS permissions FERB uses (TCC services: the setup rows' panes, kb area app), and FERB's two app IDs:
    /// FERB.app and its background agent. Nothing of another app is named.
    public static let services = ["Accessibility", "ScreenCapture", "ListenEvent", "SystemPolicyAllFiles", "Microphone",
                                  "SpeechRecognition", "Pasteboard", "AppleEvents"]
    public static let ownBundleIDs = ["com.ferb.FERB", "com.ferb.daemon"]

    /// `tccutil` arguments clearing each of FERB's own entries once, so macOS asks cleanly for the new signer instead of
    /// keeping inert switches for the old one (APP-902).
    public static func permissionResets() -> [[String]] {
        ownBundleIDs.flatMap { bundle in services.map { ["reset", $0, bundle] } }
    }
}

/// FERB.app's own registration of its background service (SMAppService, DMN-587) and when it falls back to the user
/// LaunchAgent that setup writes (DMN-513's path, kept).
public enum AppServicePlan {
    public enum Next: Equatable {
        case waitForFERB             // macOS lists it on: wait until the daemon answers
        case approval                // the owner has FERB switched off in Login Items: guide that visit
        case fallback(String)        // the LaunchAgent instead, and why
    }

    /// macOS said the owner declined the job (kSMErrorLaunchDeniedByUser): their Login Items switch, not a failure.
    public struct DeniedByUser: LocalizedError {
        public init() {}
        public var errorDescription: String? { "FERB is switched off in Login Items" }
    }

    /// After `register()`: `error` its failure in words (nil: it returned, or the service was already registered),
    /// `deniedByUser` the owner's switch, `status` what macOS lists now. Waiting on approval is the owner's switch, so
    /// it is guided, never worked around; a throw or a status that stays off falls back.
    public static func afterRegister(error: String?, deniedByUser: Bool, status: ServiceStatus) -> Next {
        if deniedByUser || status == .requiresApproval { return .approval }
        if status == .enabled { return .waitForFERB }
        return .fallback(error ?? "macOS left it \(status == .notFound ? "unknown" : "off")")
    }

    /// The service line while the fallback runs (the tour's service step shows it; the record says launch_agent).
    public static func fallbackLine(_ why: String) -> String {
        "macOS didn't start FERB's background service from inside the app (\(why)), so FERB runs it as a login " +
            "agent instead."
    }

    public static let notStarted = "FERB didn't answer within 30 seconds"
}
