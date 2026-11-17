import FerbMobile
import os

/// The app's own lines in the system log, category `ferb-app` (kb PHN-641; Android's AppLog, tag `ferb-app`): what
/// the lock and the Mac's unlock did and how long each took, and the microphone's states, so a report from a real
/// phone can be read from Console. Never a secret, a path, a message's text or the words spoken: only states, codes,
/// counts and milliseconds.
public enum AppLog {
    private static let log = Logger(subsystem: "com.ferb.phone", category: "ferb-app")

    public static func i(_ line: String) {
        log.info("\(line, privacy: .public)")
    }

    /// An error's kind only (its case name), never its detail text.
    static func kind(_ e: FerbError) -> String {
        String(describing: e).split(separator: "(").first.map(String.init) ?? "error"
    }
}
