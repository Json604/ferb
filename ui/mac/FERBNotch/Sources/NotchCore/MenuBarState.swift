import Foundation

/// Whether the notch app reaches the daemon (L34 D). One refused connection isn't "not running": launchd restarts a
/// crashed daemon within its 5 s throttle, and the connect loop retries every 2 s, so FERB counts as not running from the
/// second refusal in a row.
public struct DaemonLink: Equatable {
    public enum State: Equatable { case connecting, connected, unreachable }

    public private(set) var state: State = .connecting
    private var misses = 0
    public static let missesBeforeNotRunning = 2

    public init() {}

    public mutating func succeeded() {
        misses = 0
        state = .connected
    }

    /// `unreachable`: the socket refused or isn't there (`DaemonRPCError.unavailable`); anything else is a dropped
    /// connection, which reconnects.
    public mutating func failed(unreachable: Bool) {
        guard unreachable else {
            misses = 0
            state = .connecting
            return
        }
        misses += 1
        state = misses >= Self.missesBeforeNotRunning ? .unreachable : .connecting
    }

    /// The owner chose Restart: connecting again until it answers.
    public mutating func restarting() {
        misses = 0
        state = .connecting
    }
}

/// The menu bar's one state, most urgent first.
public enum MenuBarState: Equatable {
    case connecting, notRunning, needsYou(Int), stopped, paused, working(Int), ready

    public static func derive(link: DaemonLink.State, paused: Bool, stopped: Bool, questions: Int,
                              running: Int) -> MenuBarState {
        switch link {
        case .connecting: return .connecting
        case .unreachable: return .notRunning
        case .connected: break
        }
        if questions > 0 { return .needsYou(questions) }
        if stopped { return .stopped }
        if paused { return .paused }
        if running > 0 { return .working(running) }
        return .ready
    }

    /// SF Symbols available on macOS 14.
    public var symbol: String {
        switch self {
        case .connecting, .ready: return "waveform"
        case .working: return "ellipsis.circle"
        case .needsYou: return "exclamationmark.bubble"
        case .paused, .stopped: return "pause.circle"
        case .notRunning: return "exclamationmark.triangle"
        }
    }

    public var statusLine: String {
        switch self {
        case .connecting: return "Connecting to FERB…"
        case .ready: return "FERB is ready"
        case .working(let n): return n == 1 ? "Working on 1 task" : "Working on \(n) tasks"
        case .needsYou(let n): return n == 1 ? "FERB needs you: 1 question" : "FERB needs you: \(n) questions"
        case .paused: return "Paused: FERB records nothing"
        case .stopped: return "Stopped: tasks are blocked"
        case .notRunning: return "FERB isn't running — Restart"
        }
    }
}

public enum MenuAction: String, Equatable {
    case openFERB, openWindow, pause, resume, liftStop, restart, settings, setUp, quit
}

public enum MenuEntry: Equatable {
    case status(String)
    case separator
    case item(String, MenuAction, key: String)
}

public enum StatusMenu {
    /// The menu (spec: Open FERB, Open FERB Window, Pause FERB, Settings…, Quit): the state line first, then what the
    /// state needs. `hasWindow`: Phase E's FERB window exists. `canSetUp`: the packaged FERB.app (Set Up FERB…).
    public static func entries(state: MenuBarState, paused: Bool, stopped: Bool, appName: String, canSetUp: Bool,
                               hasWindow: Bool) -> [MenuEntry] {
        var entries: [MenuEntry] = [.status(state.statusLine), .separator, .item("Open FERB", .openFERB, key: "")]
        if hasWindow { entries.append(.item("Open FERB Window", .openWindow, key: "")) }
        if state == .notRunning {
            entries.append(.item("Restart FERB", .restart, key: ""))
        } else if state != .connecting {
            if stopped { entries.append(.item("Lift Emergency Stop", .liftStop, key: "")) }
            entries.append(paused ? .item("Resume FERB", .resume, key: "") : .item("Pause FERB", .pause, key: ""))
        }
        entries.append(.item("Settings…", .settings, key: ","))
        if canSetUp { entries.append(.item("Set Up FERB…", .setUp, key: "")) }
        entries += [.separator, .item("Quit \(appName)", .quit, key: "q")]
        return entries
    }
}
