import Foundation

/// Who signed FERB.app (T-27), in the words the setup record keeps (ferb-device install.rs `Installer::signer`):
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

/// FERB.app's own registration of its background service (SMAppService), the one way the Rust build runs it
/// (DMN-593): no LaunchAgent around it and nothing moved from an older build. When macOS refuses it or launchd can't
/// start it, the owner is told why and how to allow it.
public enum AppServicePlan {
    /// Whether this FERB.app registers its own service: every packaged FERB.app without Python inside, that is the
    /// Rust build, whoever signed it (a Developer ID release, or a developer's ad-hoc build: in the Rust build the
    /// internal commands never write a LaunchAgent, DMN-593). The Python build (Contents/Resources/python) keeps its
    /// user LaunchAgent (DMN-513); a development build (`swift run`) registers nothing.
    public static func registersInApp(packaged: Bool, holdsPython: Bool) -> Bool { packaged && !holdsPython }

    /// Registered afresh (unregistered, then registered) when setup runs: a build signed without a Team ID, whose
    /// registration's launch constraint names one exact build (DMN-513's why, APP-599's rule). A Developer ID
    /// registration is kept across builds: its constraint names the team.
    public static func registersAfresh(signer: String) -> Bool { !Signer.isTeam(signer) }

    public enum Next: Equatable {
        case waitForFERB             // macOS lists it on: wait until the daemon answers
        case approval                // the owner has FERB switched off in Login Items: guide that visit
        case refused(String)         // macOS refused it: the owner is told why and how to allow it
    }

    /// macOS said the owner declined the job (kSMErrorLaunchDeniedByUser): their Login Items switch, not a failure.
    public struct DeniedByUser: LocalizedError {
        public init() {}
        public var errorDescription: String? { "FERB is switched off in Login Items" }
    }

    /// After `register()`: `error` its failure in words (nil: it returned, or the service was already registered),
    /// `deniedByUser` the owner's switch, `status` what macOS lists now. Waiting on approval is the owner's switch, so
    /// it is guided; a throw or a status that stays off is a refusal, said with its reason. Never anything around it.
    public static func afterRegister(error: String?, deniedByUser: Bool, status: ServiceStatus) -> Next {
        if deniedByUser || status == .requiresApproval { return .approval }
        if status == .enabled { return .waitForFERB }
        return .refused(error ?? "macOS left it \(status == .notFound ? "unknown" : "off")")
    }

    /// What the tour's service step says when macOS refused the registration or launchd can't start it: the reason,
    /// then the Login Items guidance (ServicePlan.approval's own words) and Try again.
    public static func refusal(_ why: String) -> String {
        "macOS didn't start FERB's background service (\(why)). Check " + ServicePlan.loginItemsPlace +
            ", then choose Try again."
    }

    public static let notStarted = "FERB didn't answer within 30 seconds"
}
