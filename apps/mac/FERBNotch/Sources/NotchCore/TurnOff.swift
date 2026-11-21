import Foundation

/// Settings → General → Turn FERB off until I open it again (APP-1110, the owner's "lets hide it in settigns"): FERB
/// stops completely (the background service, with the phone link's helper and the browsers' relays it runs) and
/// FERB.app quits; opening FERB.app again starts everything through its usual launch. FERB.app writes `offFile` in
/// FERB's home and sends launchd's SIGTERM: while the file is there the daemon's stop is FERB's own (exit 0, so launchd
/// leaves it, DMN-619) and a daemon launchd starts at a login exits at once. FERB.app removes the file as it opens.
public enum TurnOffPlan {
    public static let action = "Turn FERB off until I open it again"
    public static let detail = "FERB stops completely, including its background work, until you open FERB.app again."
    public static let confirm = "Turn Off"
    /// FERB couldn't write the file that keeps it off: nothing was stopped.
    public static let failed = "FERB couldn't turn off. Nothing was stopped; try again."

    /// The question before turning off while work runs; nil when nothing runs (FERB turns off at once).
    public static func question(running: Int) -> String? {
        switch running {
        case ..<1: return nil
        case 1: return "A task is running. Stop it and turn FERB off?"
        default: return "\(running) tasks are running. Stop them and turn FERB off?"
        }
    }

    /// In FERB's home (ferb-device install.rs `turned_off_marker`).
    public static let offFile = "service.off"

    /// launchd's stop of FERB's job (Installer::stop's): SIGTERM to the daemon it runs.
    public static func stop(uid: uid_t) -> [String] { ["kill", "SIGTERM", "gui/\(uid)/\(LaunchctlPlan.label)"] }

    /// What `launchctl print gui/<uid>/<label>` says runs: the daemon's pid, nil when none runs (or nothing printed).
    public static func pid(_ printed: String) -> Int32? {
        for line in printed.split(separator: "\n") {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("pid = ") { return Int32(trimmed.dropFirst("pid = ".count)) }
        }
        return nil
    }
}
