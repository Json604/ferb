import Foundation

/// FERB's automatic updates (L34 Phase G): what the app reads from its own Info.plist and the daemon. Sparkle checks
/// the public feed and accepts an update whose EdDSA signature matches FERB's key (Sparkle 2 accepts either the same
/// EdDSA key or the same code-signing identity, so the Developer ID switch passes on the key alone; plan §1); the
/// daemon says whether a restart would cut work.
public enum UpdatePlan {
    public static let feedPrefix = "https://github.com/Json604/ferb/releases/latest/download/appcast-"

    /// An update Sparkle marks as a major upgrade (the appcast's minimumAutoupdateVersion is newer than this build:
    /// the switch to the Developer ID, REL-423) never installs in the background: FERB asks first (APP-903).
    public static func askLine(version: String) -> String {
        "FERB \(version) is ready. Update now? You'll grant FERB's permissions once more."
    }

    /// The feed a release build checks: FERB's own, with FERB's public key beside it; nil for any other build.
    public static func feedURL(_ info: [String: Any]) -> String? {
        guard let feed = info["SUFeedURL"] as? String, feed.hasPrefix(feedPrefix), feed.hasSuffix(".xml"),
              let key = info["SUPublicEDKey"] as? String, !key.isEmpty else { return nil }
        return feed
    }

    public enum Prepared: Equatable {
        case ready
        case waiting([String])
    }

    /// `update.prepare`'s reply: FERB stopped for the update, or the work it waits for, as the owner reads it.
    public static func prepared(_ reply: [String: Any]) -> Prepared {
        if reply["ready"] as? Bool == true { return .ready }
        let work = (reply["running"] as? [[String: Any]] ?? []).map { row -> String in
            if let goal = row["goal"] as? String, !goal.isEmpty { return goal }
            return row["task_id"] as? String ?? "a task"
        }
        return .waiting(work.isEmpty ? ["a task"] : work)
    }

    public static func waitingLine(version: String, work: [String]) -> String {
        "FERB \(version) installs when this finishes: " + work.joined(separator: "; ")
    }

    /// Whether Sparkle's abort is a failure the owner should see: not "no update found" (SUNoUpdateError, reported as
    /// an abort too) nor an install the owner cancelled (SUInstallationCancelledError).
    public static func abortIsFailure(domain: String, code: Int) -> Bool {
        !(domain == "SUSparkleErrorDomain" && (code == 1001 || code == 4007))
    }

    /// The line the notch shows once after the first launch of a new version (APP-749); nil on a first launch, a
    /// relaunch, or a reinstall or rebuild of the same version (each build has its own CFBundleVersion). A FERB that
    /// recorded only its build (before APP-749) gives no version: the line shows unless the chat already has it
    /// (`alreadySaid`).
    public static func updatedNote(previousVersion: String?, previousBuild: String?, build: String, version: String,
                                   alreadySaid: Bool) -> String? {
        guard !version.isEmpty else { return nil }
        if let previousVersion, !previousVersion.isEmpty {
            return previousVersion == version ? nil : "Updated to \(version)"
        }
        guard let previousBuild, !previousBuild.isEmpty, previousBuild != build, !alreadySaid else { return nil }
        return "Updated to \(version)"
    }
}

/// The last update check, download or install Sparkle finished, kept in FERB.app's defaults (`FERBUpdateLastResult`)
/// until the next one, so a failure is seen in Settings → General → Updates instead of leaving no trace (APP-658).
public struct UpdateResult: Equatable {
    public static let key = "FERBUpdateLastResult"
    public let at: Date
    public let failed: Bool
    public let text: String

    public init(at: Date, failed: Bool, text: String) {
        self.at = at
        self.failed = failed
        self.text = text
    }

    public init?(_ value: Any?) {
        guard let row = value as? [String: Any], let at = row["at"] as? Date, let text = row["text"] as? String else {
            return nil
        }
        self.init(at: at, failed: row["failed"] as? Bool ?? false, text: text)
    }

    public var stored: [String: Any] { ["at": at, "failed": failed, "text": text] }
}
