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

    /// Whether to register now: the bundle carries the helper (its plist and program on disk) and macOS doesn't list it
    /// (`.notRegistered`, or `.notFound`, which macOS also reports for a daemon never registered). One waiting for
    /// approval or already on is left as it is.
    public static func registers(_ status: ServiceStatus, inBundle: Bool) -> Bool {
        inBundle && (status == .notRegistered || status == .notFound)
    }

    /// A mode session that is on tries once, whoever started it (a phone start never opens the Mac's warning), while
    /// the daemon doesn't have the helper ready.
    public static func triesForSession(_ status: InsomniaStatus, tried: String?) -> Bool {
        status.on && status.closedLid != "ready" && status.since != tried
    }

    /// What the warning says for macOS's status after that.
    public static func closedLid(_ status: ServiceStatus, inBundle: Bool) -> ClosedLid {
        guard inBundle else { return .sleeps }
        return status == .enabled ? .keepsWorking : .needsApproval
    }

    /// The helper's state as `insomnia.report {"event": "helper"}` names it, for macOS's status.
    public static func reported(_ status: ServiceStatus) -> String {
        switch status {
        case .enabled: return "enabled"
        case .requiresApproval: return "requires_approval"
        case .notRegistered, .notFound: return "registered"
        }
    }

    /// An error's words for the daemon and the log: "<description> (<domain> <code>)".
    public static func describe(_ error: Error) -> String {
        let ns = error as NSError
        return "\(ns.localizedDescription) (\(ns.domain) \(ns.code))"
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

    /// The warning's closed-lid line: the daemon's `lid_line`; from an older daemon, FERB.app's own words, which never
    /// say a closed lid keeps working unless the daemon says the helper is ready.
    public static func lidLine(_ status: InsomniaStatus?, local: PowerHelperPlan.ClosedLid) -> String {
        if let line = status?.lidLine { return line }
        let ready = status?.closedLid == "ready"
        return closedLid(local == .keepsWorking && !ready ? .sleeps : local)
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
