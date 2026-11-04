import Foundation

/// macOS's "Quit & Reopen" after a privacy switch, made reliable (APP-661). System Settings sends FERB a Quit Apple
/// Event and relaunches it about three seconds after FERB has exited, from its Privacy & Security extension; when the
/// owner closes System Settings within that window the relaunch never comes (the owner's run, 2026-11-04 08:07:03:
/// FERB quit, System Settings closed 2.2 s later, no relaunch). So a quit System Settings asked for leaves a small
/// detached watcher behind: once FERB has exited it waits while the process that sent the quit lives (macOS may still
/// relaunch FERB) and then opens FERB.app only if nothing has.
public enum PermissionRelaunch {
    /// System Settings or one of its extensions (Privacy & Security is `com.apple.settings.PrivacySecurity.extension`):
    /// the processes that send Quit & Reopen's quit.
    public static func isSystemSettings(_ bundleID: String?) -> Bool {
        guard let bundleID else { return false }
        return bundleID == SettingsAnchor.bundleID || bundleID.hasPrefix("com.apple.settings.")
    }

    /// A quit to reopen after: System Settings sent it, and it isn't a log out, restart or shut down (a quit that
    /// carries a reason).
    public static func reopensAfter(senderBundleID: String?, hasQuitReason: Bool) -> Bool {
        isSystemSettings(senderBundleID) && !hasQuitReason
    }

    /// The watcher, run detached with /bin/sh: $1 FERB's pid, $2 the sender's pid, $3 FERB's executable, $4 FERB.app,
    /// $5 seconds between looks, $6 what opens the app (`/usr/bin/open -g` in the app; tests give their own).
    public static let script = """
        while kill -0 "$1" 2>/dev/null; do sleep "$5"; done
        running() { /bin/ps -axo comm= | /usr/bin/grep -Fqx -- "$3"; }
        while kill -0 "$2" 2>/dev/null; do running && exit 0; sleep "$5"; done
        running || exec $6 "$4"
        """

    /// The watcher's command line: `/bin/sh -c script ferb-relaunch <args>`.
    public static func arguments(ferbPID: pid_t, senderPID: pid_t, executable: String, app: String,
                                 poll: Double, opener: String = "/usr/bin/open -g") -> [String] {
        ["/bin/sh", "-c", script, "ferb-relaunch", String(ferbPID), String(senderPID), executable, app,
         String(poll), opener]
    }
}
