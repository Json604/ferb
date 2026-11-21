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
    case connecting, settingUp, notRunning, needsYou(Int), stopped, paused, working(Int), ready

    /// `settingUp`: FERB.app hasn't set the background service up yet, or is installing, starting or restarting it
    /// now (a first run before Meet, a quiet update, a permission's restart): a daemon not answering then is expected,
    /// never "isn't running" (APP-698). Only a service that should run and doesn't is `notRunning`.
    public static func derive(link: DaemonLink.State, paused: Bool, stopped: Bool, questions: Int,
                              running: Int, settingUp: Bool = false) -> MenuBarState {
        switch link {
        case .connecting: return settingUp ? .settingUp : .connecting
        case .unreachable: return settingUp ? .settingUp : .notRunning
        case .connected: break
        }
        if questions > 0 { return .needsYou(questions) }
        if stopped { return .stopped }
        if paused { return .paused }
        if running > 0 { return .working(running) }
        return .ready
    }

    /// The menu bar icon for this state: FERB's "f." whose dot says the state (APP-961). The one place the states map
    /// to the icon; FERBNotch's MenuBarIcon draws it.
    public var mark: MenuBarMark {
        switch self {
        case .ready: return MenuBarMark(dot: .solid, dimmed: false)
        case .working: return MenuBarMark(dot: .lit, dimmed: false)
        case .needsYou: return MenuBarMark(dot: .badge, dimmed: false)
        case .connecting, .settingUp: return MenuBarMark(dot: .hollow, dimmed: false)
        case .paused: return MenuBarMark(dot: .pause, dimmed: true)
        case .stopped: return MenuBarMark(dot: .stop, dimmed: true)
        case .notRunning: return MenuBarMark(dot: .hollow, dimmed: true)
        }
    }

    public var statusLine: String {
        switch self {
        case .connecting: return "Connecting to FERB…"
        case .settingUp: return "Setting up FERB…"
        case .ready: return "FERB is ready"
        case .working(let n): return n == 1 ? "Working on 1 task" : "Working on \(n) tasks"
        case .needsYou(let n): return n == 1 ? "FERB needs you: 1 question" : "FERB needs you: \(n) questions"
        case .paused: return "Paused: FERB records nothing"
        case .stopped: return "Stopped: tasks are blocked"
        case .notRunning: return "FERB isn't running — Restart"
        }
    }
}

/// FERB's menu bar icon (APP-960, APP-961, APP-963): the logo's "f" and its dot, a template image, drawn one of two
/// ways (`Style`). The dot carries the state, as the wordmark's dot and the phone apps' lamp do; when FERB isn't doing
/// anything for its owner (paused, stopped, not running) the mark goes quiet (`dimmed`).
public struct MenuBarMark: Hashable, Sendable {
    /// The two drawings of the logo (APP-962): the app icon's tile in one colour with the f and dot cut out of it, or
    /// the bare letter with its dot after it.
    public enum Style: Hashable, Sendable, CaseIterable { case tile, letter }

    /// The drawing FERB.app uses. One line switches it; the guide's words follow.
    public static let style: Style = .letter

    public enum Dot: Hashable, Sendable, CaseIterable {
        /// The logo's dot: ready.
        case solid
        /// The dot inside a ring, a lamp that is lit: working.
        case lit
        /// A round badge at the top right, cut clear of the mark, as a badge sits on an app icon: needs you.
        case badge
        /// The dot's outline only, not lit: connecting, setting up, not running.
        case hollow
        /// Two bars where the dot was: paused.
        case pause
        /// A square where the dot was: the emergency stop.
        case stop
    }

    public let dot: Dot
    /// The quiet mark: the tile as an outline, or the letter faded. The dot's shape stays full so the state reads.
    public let dimmed: Bool

    public init(dot: Dot, dimmed: Bool) {
        self.dot = dot
        self.dimmed = dimmed
    }
}

extension MenuBarMark.Style {
    /// The mark itself, in the guide's words.
    public var described: String {
        switch self {
        case .tile:
            return "a small rounded square with FERB's f and its dot cut out of it, FERB's app icon in one colour"
        case .letter: return "a lowercase f with a dot after it, FERB's own mark"
        }
    }

    /// The guide's title for the quiet marks.
    public var quietTitle: String {
        switch self {
        case .tile: return "Read FERB's menu bar icon when it is only an outline"
        case .letter: return "Read FERB's menu bar icon when its f is faded"
        }
    }
}

extension MenuBarMark {
    /// How the mark looks, in the guide's words (APP-963), for the drawing in use.
    public var look: String { look(in: Self.style) }

    public func look(in style: MenuBarMark.Style) -> String {
        let dot: String
        switch (style, self.dot) {
        case (.tile, .solid): dot = "the dot cut out"
        case (.tile, .lit): dot = "the dot cut out inside a cut-out ring"
        case (.tile, .badge): dot = "a round badge on the square's top-right corner"
        case (.tile, .hollow): dot = dimmed ? "an empty dot" : "only a ring cut out for the dot"
        case (.tile, .pause): dot = "two short bars"
        case (.tile, .stop): dot = "a small square"
        case (.letter, .solid): dot = "a solid dot"
        case (.letter, .lit): dot = "the dot inside a ring"
        case (.letter, .badge): dot = "the dot raised to the top right, like a badge"
        case (.letter, .hollow): dot = "an empty, outlined dot"
        case (.letter, .pause): dot = "two short bars instead of the dot"
        case (.letter, .stop): dot = "a small square instead of the dot"
        }
        guard dimmed else { return dot }
        switch style {
        case .tile: return "the square as an outline with \(dot)"
        case .letter: return "a faded f with \(dot)"
        }
    }
}

extension MenuBarState {
    /// Every state, once, counts aside: FERB's guide describes each (a test checks `derive` gives no other).
    public static let every: [MenuBarState] = [.ready, .working(1), .needsYou(1), .connecting, .settingUp, .paused,
                                                .stopped, .notRunning]

    /// What the state means, in the guide's words: FERB explains its menu bar icon from these (APP-963).
    public var meaning: String {
        switch self {
        case .ready: return "FERB is ready"
        case .working: return "FERB is working on a task"
        case .needsYou: return "FERB has a question or needs your OK; open FERB to answer it"
        case .connecting: return "FERB is connecting to its background service"
        case .settingUp: return "FERB is setting up its background service, after an install or an update"
        case .paused: return "FERB is paused and records nothing; Resume FERB is in the icon's menu"
        case .stopped:
            return "the emergency stop is on and tasks are blocked; Lift Emergency Stop is in the icon's menu"
        case .notRunning: return "FERB's background service isn't running; Restart FERB is in the icon's menu"
        }
    }
}

/// FERB's how-tos about its menu bar icon, made from the map that draws it (MenuBarState.mark, `look`, `meaning`) and
/// the pill-hidden colours (StatusTint), so the words can't drift from the drawing (APP-963). AppGuide carries them.
public enum MenuBarGuide {
    public static var howtos: [AppGuide.HowTo] {
        let colours = [(SessionPhase.working, "while FERB works"), (.attention, "when it needs you"),
                       (.result, "when a task is done")]
            .compactMap { phase, when in StatusTint.of(phase, pillHidden: true).map { "\($0.name) \(when)" } }
        return [
            AppGuide.HowTo(title: "Find FERB's menu bar icon",
                           steps: "In the menu bar at the top right of the screen: \(MenuBarMark.style.described). "
                               + "Click it for FERB's menu: open FERB, pause it, Settings, quit. With "
                               + "the pill hidden in window mode the icon is also coloured: "
                               + colours.joined(separator: ", ") + "."),
            AppGuide.HowTo(title: "Read the dot on FERB's menu bar icon", steps: lines(dimmed: false)),
            AppGuide.HowTo(title: MenuBarMark.style.quietTitle, steps: lines(dimmed: true)),
        ]
    }

    /// One sentence per look, in `every`'s order; states that look the same share it.
    private static func lines(dimmed: Bool) -> String {
        var looks: [MenuBarMark] = []
        for state in MenuBarState.every where state.mark.dimmed == dimmed && !looks.contains(state.mark) {
            looks.append(state.mark)
        }
        return looks.map { mark in
            let meanings = MenuBarState.every.filter { $0.mark == mark }.map(\.meaning)
            let look = mark.look.prefix(1).uppercased() + mark.look.dropFirst()
            return "\(look): \(meanings.joined(separator: ", or "))."
        }.joined(separator: " ")
    }
}

extension StatusTint {
    /// The colour's name, as the guide says it.
    public var name: String {
        switch self {
        case .amber: return "amber"
        case .red: return "red"
        case .green: return "green"
        }
    }
}

public enum MenuAction: String, Equatable {
    case openFERB, openWindow, pause, resume, liftStop, restart, settings, setUp, quit, installUpdate
    /// Insomnia mode (APP-980): the warning, or the owner's own end.
    case keepWorking, endKeepWorking
}

public enum MenuEntry: Equatable {
    case status(String)
    case separator
    case item(String, MenuAction, key: String)
}

public enum StatusMenu {
    /// The menu (spec: Open FERB, Open FERB Window, Pause FERB, Settings…, Quit): the state line first, then what the
    /// state needs. `hasWindow`: Phase E's FERB window exists. `canSetUp`: the packaged FERB.app (Set Up FERB…,
    /// which replays the first-run intro: kb area app). `keepingAwake`: insomnia mode as the daemon last said (nil: a
    /// daemon that doesn't have it, or none read yet: no item).
    public static func entries(state: MenuBarState, paused: Bool, stopped: Bool, appName: String, canSetUp: Bool,
                               hasWindow: Bool, updateOffer: String? = nil, updateAsks: Bool = false,
                               keepingAwake: Bool? = nil, helperProblem: String? = nil) -> [MenuEntry] {
        var entries: [MenuEntry] = [.status(state.statusLine), .separator, .item("Open FERB", .openFERB, key: "")]
        if hasWindow { entries.append(.item("Open FERB Window", .openWindow, key: "")) }
        if state == .notRunning {
            entries.append(.item("Restart FERB", .restart, key: ""))
        } else if state != .connecting && state != .settingUp {
            if stopped { entries.append(.item("Lift Emergency Stop", .liftStop, key: "")) }
            entries.append(paused ? .item("Resume FERB", .resume, key: "") : .item("Pause FERB", .pause, key: ""))
            if let keepingAwake {             // APP-980: the warning first; on, the owner's end
                entries.append(keepingAwake ? .item(InsomniaWords.menuEnd, .endKeepWorking, key: "")
                                            : .item(InsomniaWords.menuStart, .keepWorking, key: ""))
                if let helperProblem { entries.append(.status(helperProblem)) }   // the root helper's problem, one line
            }
        }
        entries.append(.item("Settings…", .settings, key: ","))
        if let updateOffer, updateAsks {      // the switch to the Developer ID asks first (APP-903)
            entries.append(.item("Update to FERB \(updateOffer)… (you'll grant FERB's permissions once more)",
                                 .installUpdate, key: ""))
        } else if let updateOffer {           // automatic updates off: an update waits for the owner's click (L34 G)
            entries.append(.item("Update available (FERB \(updateOffer)) — Install", .installUpdate, key: ""))
        }
        if canSetUp { entries.append(.item("Set Up FERB…", .setUp, key: "")) }
        entries += [.separator, .item("Quit \(appName)", .quit, key: "q")]
        return entries
    }
}
