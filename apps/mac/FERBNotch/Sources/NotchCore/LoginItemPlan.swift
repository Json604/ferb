import Foundation

/// FERB.app opening at login (L34 Phase F). macOS's own login item (`SMAppService.mainApp`) is tried first; when
/// macOS refuses it for FERB's self-made signature, a user LaunchAgent opens the app instead. The background service
/// already starts at login on its own (`com.ferb.daemon`).
public enum LoginItemPlan {
    /// The fallback LaunchAgent's label and file name (`~/Library/LaunchAgents/<label>.plist`).
    public static let label = "com.ferb.FERB.login"

    public static let approvalNote = "macOS needs your OK: System Settings → General → Login Items → allow FERB."

    public static func fallbackNote(_ refusal: String?) -> String {
        "macOS refused FERB's login item" + (refusal.map { " (\($0))" } ?? "") +
            ", so a login LaunchAgent opens FERB instead."
    }

    /// macOS returned from `register()` without turning the item on (and without asking for the owner's OK).
    public struct Refused: LocalizedError {
        public init() {}
        public var errorDescription: String? { "the login item didn't turn on" }
    }

    /// Opens FERB.app once per login, in the background (rule 17), in the owner's GUI session only; System Settings
    /// lists it under FERB.
    public static func agent(appPath: String) -> [String: Any] {
        ["Label": label, "ProgramArguments": ["/usr/bin/open", "-g", "-a", appPath], "RunAtLoad": true,
         "LimitLoadToSessionType": "Aqua", "AssociatedBundleIdentifiers": ["com.ferb.FERB"]]
    }

    /// A plist FERB wrote (anything else at that path is the owner's and is left alone).
    public static func isOurs(_ plist: [String: Any]?) -> Bool { plist?["Label"] as? String == label }

    /// On by default once: after that, only the owner's switch decides.
    public static func shouldApplyDefault(applied: Bool) -> Bool { !applied }
}
