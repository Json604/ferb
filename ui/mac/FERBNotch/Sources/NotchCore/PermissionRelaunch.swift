import Foundation

/// macOS's "Quit & Reopen" after a privacy switch, made reliable (APP-684). System Settings sends FERB a Quit
/// Apple Event and may relaunch it about three seconds after FERB has exited, from its Privacy & Security extension;
/// when the owner closes System Settings within that window the relaunch never comes (the owner's run, 2026-11-04
/// 08:07:03), and after Full Disk Access's quit it didn't come at all (09:32:17: FERB was back only when the owner
/// opened it, 31 s later). So a quit System Settings asked for leaves a small detached watcher behind: once FERB has
/// exited it waits while the process that sent the quit lives, for at most `reads` looks (macOS may still relaunch
/// FERB), and then opens FERB.app only if nothing has.
public enum PermissionRelaunch {
    /// System Settings or one of its extensions (Privacy & Security is `com.apple.settings.PrivacySecurity.extension`):
    /// the processes that send Quit & Reopen's quit.
    public static func isSystemSettings(_ bundleID: String?) -> Bool {
        guard let bundleID else { return false }
        return bundleID == SettingsAnchor.bundleID || bundleID.hasPrefix("com.apple.settings.")
    }

    /// A four-character Apple Event code as its number.
    public static func code(_ text: String) -> UInt32 {
        text.utf8.prefix(4).reduce(0) { $0 << 8 | UInt32($1) }
    }

    /// The code as its four characters, for the log.
    public static func text(_ code: UInt32) -> String {
        String(decoding: [24, 16, 8, 0].map { UInt8(truncatingIfNeeded: code >> UInt32($0)) }, as: UTF8.self)
    }

    /// The quit reasons (kAEQuitReason) of a log out, restart or shut down (AERegistry.h: kAELogOut, kAEReallyLogOut,
    /// kAEShowRestartDialog, kAEShowShutdownDialog, kAERestart, kAEShutDown): the session ends and nothing reopens.
    public static let sessionEnds: Set<UInt32> = Set(["logo", "rlgo", "rrst", "rsdn", "rest", "shut"].map(code))

    /// A quit to reopen after: System Settings sent it and it isn't a log out, restart or shut down. Any other quit
    /// reason still counts (APP-684): only System Settings' presence of a reason had kept the watcher from starting.
    public static func reopensAfter(senderBundleID: String?, quitReason: UInt32?) -> Bool {
        isSystemSettings(senderBundleID) && !(quitReason.map(sessionEnds.contains) ?? false)
    }

    /// The watcher, run detached with /bin/sh: $1 FERB's pid, $2 the sender's pid, $3 FERB's executable, $4 FERB.app,
    /// $5 seconds between looks, $6 what opens the app (`/usr/bin/open -g` in the app; tests give their own), $7 the
    /// most looks it waits on the sender.
    public static let script = """
        while kill -0 "$1" 2>/dev/null; do sleep "$5"; done
        running() { /bin/ps -axo comm= | /usr/bin/grep -Fqx -- "$3"; }
        n=0
        while kill -0 "$2" 2>/dev/null && [ "$n" -lt "$7" ]; do running && exit 0; sleep "$5"; n=$((n+1)); done
        running || exec $6 "$4"
        """

    /// The watcher's command line: `/bin/sh -c script ferb-relaunch <args>`. `reads`: how many looks macOS's own
    /// relaunch gets while the sender lives: the grant wait's three unanswered reads before it starts the service
    /// (APP-598, SetupModel.serviceStartAfter), about 4.5 s at the grant poll, past macOS's ~3 s.
    public static func arguments(ferbPID: pid_t, senderPID: pid_t, executable: String, app: String,
                                 poll: Double, reads: Int = 3, opener: String = "/usr/bin/open -g") -> [String] {
        ["/bin/sh", "-c", script, "ferb-relaunch", String(ferbPID), String(senderPID), executable, app,
         String(poll), opener, String(reads)]
    }
}
