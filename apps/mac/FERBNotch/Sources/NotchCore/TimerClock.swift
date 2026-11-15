import Foundation

/// A timer's clock (APP-762): what the timer card, the closed notch's live activity and the done alert show, from the
/// timer's start and end as FERB's records give them. Apple's Clock app is the model: a countdown in monospaced digits
/// and a ring that drains linearly from full to empty over the timer's duration.
public enum TimerClock {
    /// The countdown as the Clock app writes it: "4:59", "59:59", "1:02:03"; whole seconds rounded up, never below 0:00.
    public static func countdown(_ seconds: TimeInterval) -> String {
        let total = max(0, Int(seconds.rounded(.up)))
        let hours = total / 3600, minutes = (total % 3600) / 60, secs = total % 60
        return hours > 0 ? String(format: "%d:%02d:%02d", hours, minutes, secs) : String(format: "%d:%02d", minutes, secs)
    }

    /// How much of the timer is left, 1 at its start and 0 at its end (the ring's fill); nil without a start.
    public static func remaining(started: Date?, ends: Date, now: Date) -> Double? {
        guard let started, ends > started else { return nil }
        return min(1, max(0, ends.timeIntervalSince(now) / ends.timeIntervalSince(started)))
    }
}

/// A timer still running, as `reminders.list` and a timer's card give it.
public struct RunningTimer: Equatable, Identifiable, Sendable {
    public let id: String
    public let name: String
    public let started: Date?
    public let ends: Date

    public init(id: String, name: String, started: Date?, ends: Date) {
        self.id = id
        self.name = name
        self.started = started
        self.ends = ends
    }

    /// A pending timer row of `reminders.list` (`kind` timer, `at`, `started`, `name`); nil for anything else.
    public init?(row: [String: Any]) {
        guard row["kind"] as? String == "timer", let id = row["id"] as? String, !id.isEmpty,
              let ends = LiveProgress.time(row["at"]) else { return nil }
        self.init(id: id, name: ShortName.of(row, fallback: "message"), started: LiveProgress.time(row["started"]),
                  ends: ends)
    }

    /// The one the closed notch shows: the soonest to end that is still running.
    public static func soonest(_ timers: [RunningTimer], now: Date) -> RunningTimer? {
        timers.filter { $0.ends > now }.min { $0.ends < $1.ends }
    }
}

/// What FERB's records say happened to a timer or reminder the app shows (DMN-548): running, cancelled or fired, and
/// the timer a fired one's Repeat started. `timers.states` gives it on connect and when a card first shows;
/// `timer.changed` keeps it after. The cards, the closed notch and the done alert's Stop and Repeat read it, never their
/// own memory.
public struct TimerRecord: Equatable, Sendable {
    public enum State: String, Sendable { case running, cancelled, fired }

    public var state: State
    public var repeatedBy: String?

    public init(state: State, repeatedBy: String? = nil) {
        self.state = state
        self.repeatedBy = repeatedBy
    }

    /// One row of `timers.states` (`state`, `repeated_by`); nil for a state this app doesn't know.
    public init?(row: [String: Any]) {
        guard let state = State(rawValue: row["state"] as? String ?? "") else { return nil }
        let repeated = row["repeated_by"] as? String
        self.init(state: state, repeatedBy: repeated?.isEmpty == false ? repeated : nil)
    }
}
