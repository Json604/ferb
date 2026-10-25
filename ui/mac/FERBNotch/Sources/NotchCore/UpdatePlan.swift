import Foundation

/// FERB's automatic updates (L34 Phase G): what the app reads from its own Info.plist and the daemon. Sparkle checks
/// the public feed and verifies FERB's EdDSA key and certificate; the daemon says whether a restart would cut work.
public enum UpdatePlan {
    public static let feedPrefix = "https://github.com/Json604/ferb/releases/latest/download/appcast-"

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

    /// The line the notch shows once after a launch on a new build; nil on a first launch or the same build.
    public static func updatedNote(previousBuild: String?, build: String, version: String) -> String? {
        guard let previousBuild, !previousBuild.isEmpty, previousBuild != build else { return nil }
        return "Updated to \(version)"
    }
}
