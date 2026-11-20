import Foundation

/// Insomnia mode's root helper (APP-986): `Contents/Helpers/ferb-power`, defined by
/// `Contents/Library/LaunchDaemons/com.ferb.power.plist` and registered by FERB.app as an SMAppService daemon, which the
/// owner approves once in System Settings → General → Login Items. It keeps a closed Mac working on battery and wakes
/// it for FERB's scheduled tasks; only ferbd calls it. What FERB.app does about it is decided here, from macOS's status.
public enum PowerHelperPlan {
    public static let plistName = "com.ferb.power.plist"
    public static let label = "com.ferb.power"

    /// What a closed lid on battery does, as the warning says it.
    public enum ClosedLid: Equatable, Sendable {
        /// The helper is approved: a closed Mac keeps working on battery.
        case keepsWorking
        /// Registered and waiting for the owner's switch in Login Items.
        case needsApproval
        /// This FERB.app carries no helper macOS can find (a development build): the Mac sleeps.
        case sleeps
    }

    /// Whether to register now, when the owner turns the mode on from the Mac: only a helper macOS doesn't list yet.
    /// One waiting for approval or already on is left as it is; one macOS can't find (no plist in this bundle) too.
    public static func registers(_ status: ServiceStatus) -> Bool { status == .notRegistered }

    /// What the warning says for macOS's status after that.
    public static func closedLid(_ status: ServiceStatus) -> ClosedLid {
        switch status {
        case .enabled: return .keepsWorking
        case .requiresApproval, .notRegistered: return .needsApproval
        case .notFound: return .sleeps
        }
    }
}

extension InsomniaWords {
    /// The warning's line under its body (APP-986), by what a closed lid on battery does.
    public static func closedLid(_ closedLid: PowerHelperPlan.ClosedLid) -> String {
        switch closedLid {
        case .keepsWorking: return "On battery with the lid closed, FERB keeps your Mac working too."
        case .needsApproval:
            return "On battery with the lid closed, your Mac sleeps until you open it. To let FERB keep it working "
                + "then too, turn on FERB in System Settings → General → Login Items."
        case .sleeps: return "On battery with the lid closed, your Mac sleeps until you open it."
        }
    }

    /// The warning's extra button while the helper waits for the owner's switch.
    public static let openLoginItems = "Open Login Items…"

    /// Uninstall stopped because macOS kept the helper on (APP-986), like the background service's words.
    public static func helperNotRemoved(_ reason: String?) -> String {
        let why = reason.map { " (\($0))" } ?? ""
        return "macOS didn't turn off FERB's helper that keeps a closed Mac working\(why), so nothing was uninstalled. "
            + "Try again, or turn FERB off in System Settings → General → Login Items first."
    }
}
