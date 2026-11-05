import Foundation
import CoreGraphics

/// The long-task card's row states (APP-718, APP-719, daemon ORCH-531/532): each plan step's state as the daemon sends
/// it (`step` events, `task.step` on the bus, a card payload's `steps`), and what a row shows from its own state, its
/// step's and whether its turn is still running. Pure: TurnCardView draws it.

/// One plan step's state: waiting (on `waitsOn`), starting, running, checking, retrying, needs_you, done, failed,
/// not_finished or wont_run. Live state only: never saved, so history never shows it.
public struct StepState: Equatable {
    public let status: String
    public let waitsOn: [String]
    public let attempt: Int
    public let startedAt: Date?
    public let endedAt: Date?
    /// The daemon's order of this step's changes; a payload's state recorded before a restart has none.
    public let seq: String?
    public let loop: Bool

    public init?(_ value: Any?) {
        guard let value = value as? [String: Any], let status = value["status"] as? String, !status.isEmpty else { return nil }
        self.status = status
        waitsOn = value["waits_on"] as? [String] ?? []
        attempt = (value["attempt"] as? NSNumber)?.intValue ?? 0
        startedAt = LiveProgress.time(value["started_at"])
        endedAt = LiveProgress.time(value["ended_at"])
        seq = (value["seq"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        loop = value["loop"] as? Bool ?? false
    }

    /// `new` replaces `current`: a state with a later seq, or any state over none; a state without a seq (from the
    /// record) only fills a step the card knows nothing of.
    public static func replaces(_ new: StepState, _ current: StepState?) -> Bool {
        guard let current else { return true }
        guard let seq = new.seq else { return false }
        guard let shown = current.seq else { return true }
        return seq > shown
    }
}

/// A loop row's round, from the task's `loop.status` (L31): how many rounds ran and when the next check is.
public struct LoopRound: Equatable {
    public let rounds: Int
    public let nextCheck: Date?
    public init(rounds: Int, nextCheck: Date?) {
        self.rounds = rounds
        self.nextCheck = nextCheck
    }
}

/// What one card row shows (APP-718): its state's words, the small spinner after them while its step works (a still
/// dot under Reduce Motion, drawn by the view), and the time its step has been working.
public struct RowDisplay: Equatable {
    public enum Kind: String {
        case waiting, starting, running, checking, retrying, needsYou, paused, done, updated, failed, wontRun
        case notFinished, loop, working, preview, previewRunning
    }

    public let kind: Kind
    /// The state's words; nil when the row's own text says it (done, updated, failed, a question).
    public let words: String?
    public let spinner: Bool
    /// Elapsed time counts from here (the step's start); nil shows none.
    public let since: Date?

    public init(kind: Kind, words: String? = nil, spinner: Bool = false, since: Date? = nil) {
        self.kind = kind
        self.words = words
        self.spinner = spinner
        self.since = since
    }

    /// Counted as running in the card's status line.
    public var isRunning: Bool { [.starting, .running, .checking, .retrying, .loop, .working].contains(kind) }
    /// Its text collapses to its first lines (APP-719): an answer or a failure; a row that needs the owner stays open.
    public var collapses: Bool { [.done, .updated, .failed, .wontRun].contains(kind) }

    /// A row's display from its own state, its step's (`steps`, by step id), the card's rows (to name a step it waits
    /// for by its row's label), whether its turn is still running (`live`: history and a dead task never show live
    /// states), an open question for it, its step's live line and its loop's round.
    public static func make(row: CardRow, steps: [String: StepState], rows: [CardRow], live: Bool,
                            hasQuestion: Bool = false, liveLine: LiveProgress? = nil, loop: LoopRound? = nil) -> RowDisplay {
        if row.state == "preview" { return RowDisplay(kind: live ? .previewRunning : .preview) }
        if hasQuestion || row.state == "asks" { return RowDisplay(kind: .needsYou) }
        if row.state == "paused" { return RowDisplay(kind: .paused) }
        if row.state == "updated" { return RowDisplay(kind: .updated) }
        if row.state == "done" { return RowDisplay(kind: .done) }
        let step = row.step.flatMap { steps[$0] }
        if row.state == "failed" {
            return step?.status == "wont_run"
                ? RowDisplay(kind: .wontRun, words: "Won't run: an earlier step couldn't be done") : RowDisplay(kind: .failed)
        }
        guard live else { return RowDisplay(kind: .notFinished, words: "Not finished") }
        guard let step else {
            // An older daemon sends no step states: the row is only known to be under way.
            return steps.isEmpty ? RowDisplay(kind: .working, words: "Working", spinner: true)
                : RowDisplay(kind: .starting, words: "Starting…", spinner: true)
        }
        if step.loop, let loop, ["starting", "running", "checking", "retrying"].contains(step.status) {
            var words = "Round \(max(1, loop.rounds))"
            if let next = loop.nextCheck { words += " · next check " + Self.clock(next) }
            return RowDisplay(kind: .loop, words: words)
        }
        switch step.status {
        case "waiting":
            var seen = Set<String>()
            let names = step.waitsOn.compactMap { dep in rows.first { $0.step == dep && !$0.direct && $0.part != row.part }?.label }
                .filter { !$0.isEmpty && seen.insert($0).inserted }
            return RowDisplay(kind: .waiting, words: names.isEmpty ? "Waiting for an earlier step"
                              : "Waiting for " + Self.list(names))
        case "starting": return RowDisplay(kind: .starting, words: "Starting…", spinner: true)
        case "running":
            return RowDisplay(kind: .running, words: liveLine.map { Self.sentence($0.text) } ?? "Working", spinner: true,
                              since: step.startedAt)
        case "retrying":     // APP-724: the attempt, then its live line and time, like a running row
            let attempt = "Trying again (attempt \(max(2, step.attempt)))"
            return RowDisplay(kind: .retrying, words: liveLine.map { attempt + " · " + $0.text } ?? attempt,
                              spinner: true, since: step.startedAt)
        case "needs_you": return RowDisplay(kind: .needsYou, words: "Needs you")
        case "not_finished": return RowDisplay(kind: .notFinished, words: "Not finished")
        case "wont_run": return RowDisplay(kind: .wontRun, words: "Won't run: an earlier step couldn't be done")
        default:
            // checking; or done / failed a moment before the row posts its answer
            return RowDisplay(kind: .checking, words: "Checking the result", spinner: true, since: step.startedAt)
        }
    }

    /// The words with the elapsed time: "reading mail.example.com · 2:41".
    public func text(at now: Date) -> String? {
        guard let words else { return nil }
        guard let since else { return words }
        return words + " · " + Self.elapsed(from: since, to: now)
    }

    /// "0:12", "2:41", "1:02:41"; clock skew never shows negative time.
    public static func elapsed(from start: Date, to now: Date) -> String {
        let seconds = max(0, Int(now.timeIntervalSince(start)))
        if seconds >= 3600 { return String(format: "%d:%02d:%02d", seconds / 3600, seconds % 3600 / 60, seconds % 60) }
        return String(format: "%d:%02d", seconds / 60, seconds % 60)
    }

    static func clock(_ date: Date) -> String {
        let formatter = DateFormatter()
        formatter.dateFormat = DateFormatter.dateFormat(fromTemplate: "jmm", options: 0, locale: .current)
        return formatter.string(from: date)
    }

    static func list(_ names: [String]) -> String {
        guard names.count > 1 else { return names.first ?? "" }
        return names.dropLast().joined(separator: ", ") + " and " + names[names.count - 1]
    }

    /// The live line as the row's words: its first letter capitalized.
    static func sentence(_ text: String) -> String {
        guard let first = text.first else { return text }
        return first.uppercased() + text.dropFirst()
    }
}

/// The running card's one status line (APP-718): "1 of 4 done · 2 running · 1 waiting · 12:03".
public enum CardStatusLine {
    public static func text(card: TurnCard, displays: [RowDisplay], since: Date?, at now: Date) -> String {
        var pieces = ["\(card.done) of \(card.total) done"]
        let running = displays.filter(\.isRunning).count
        let waiting = displays.filter { $0.kind == .waiting }.count
        let asking = displays.filter { $0.kind == .needsYou || $0.kind == .paused }.count
        if running > 0 { pieces.append("\(running) running") }
        if waiting > 0 { pieces.append("\(waiting) waiting") }
        if asking > 0 { pieces.append("\(asking) need\(asking == 1 ? "s" : "") you") }
        if let since { pieces.append(RowDisplay.elapsed(from: since, to: now)) }
        return pieces.joined(separator: " · ")
    }
}

/// Collapsed text (APP-719, display only): a done or failed row shows its first ~3 lines, a long answer in chat its
/// first ~6, with the rest one click away; the same text, never a second version.
public enum CollapsePolicy {
    public static let rowLines = 3
    public static let answerLines = 6

    public static func clampHeight(lines: Int, lineHeight: CGFloat) -> CGFloat { CGFloat(lines) * lineHeight }

    /// The control shows only when the text is taller than the clamp by more than half a line.
    public static func needsControl(natural: CGFloat, clamp: CGFloat, lineHeight: CGFloat) -> Bool {
        natural > clamp + lineHeight / 2
    }
}

/// Which collapsed texts the owner opened, by (turn, part or answer). Kept while FERB.app runs (it survives the card's
/// updates); a relaunch starts collapsed.
public struct ExpansionState: Equatable {
    public private(set) var open: Set<String> = []
    public init() {}

    public static func key(turn: UUID, item: String) -> String { turn.uuidString + "/" + item }
    public func isOpen(_ key: String) -> Bool { open.contains(key) }
    public mutating func toggle(_ key: String) {
        if open.remove(key) == nil { open.insert(key) }
    }
}
