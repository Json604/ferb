import Foundation
import CoreGraphics

/// The answer comes forward (APP-720): what the chat transcript does when something arrives. The model asks for an
/// item by its id (a `TranscriptScrollRequest`); the transcript decides, from where the owner is and what they did,
/// whether to bring the item's top into view, show a pill naming it, or leave the view alone. Pure state: the
/// transcript (ConversationTranscript) measures and scrolls.

/// The ids the transcript gives the items a request can name.
public enum TranscriptTarget {
    public static func row(turn: UUID, part: String) -> String { "row-\(turn.uuidString)-\(part)" }
    public static func footer(turn: UUID) -> String { "footer-\(turn.uuidString)" }
    public static func answer(turn: UUID) -> String { "answer-\(turn.uuidString)" }
    public static func question(_ id: String) -> String { "question-\(id)" }
}

/// One request to bring an item forward. `revealing`: the notch body was hidden when it arrived (FERB opens it to show
/// the item), so the transcript lands on the item without animating.
public struct TranscriptScrollRequest: Equatable {
    public enum Reason: Int, Comparable {
        /// A row filled quietly: never moves the view; a pill when it is out of sight.
        case quietFill
        /// The owner collapsed an item: its top stays in view.
        case collapse
        case answer
        case taskDone
        case needsYou

        public static func < (lhs: Reason, rhs: Reason) -> Bool { lhs.rawValue < rhs.rawValue }
    }

    public let target: String
    public let reason: Reason
    /// What a pill calls it (a row's label).
    public let label: String
    public let generation: Int
    public let revealing: Bool

    public init(target: String, reason: Reason, label: String = "", generation: Int, revealing: Bool) {
        self.target = target
        self.reason = reason
        self.label = label
        self.generation = generation
        self.revealing = revealing
    }
}

/// A pill naming what arrived out of view; tapping it brings that item forward.
public struct TranscriptPill: Equatable {
    public let reason: TranscriptScrollRequest.Reason
    public let target: String
    public let label: String
    /// Several questions: how many, and the target stays the oldest.
    public var count: Int
    /// The item is above the view.
    public let up: Bool

    public var text: String {
        switch reason {
        case .needsYou: return count > 1 ? "FERB asks you · \(count)" : "FERB asks you"
        case .taskDone: return "Task done"
        case .answer: return "New answer"
        case .quietFill, .collapse: return label.isEmpty ? "A part is done" : "\(label) is done"
        }
    }

    /// The pill after `new` arrived: questions count up and keep the oldest target; otherwise the more urgent stays.
    public static func merged(_ current: TranscriptPill?, _ new: TranscriptPill) -> TranscriptPill {
        guard let current else { return new }
        if current.reason == .needsYou && new.reason == .needsYou {
            var counted = current
            counted.count += new.count
            return counted
        }
        return new.reason >= current.reason ? new : current
    }
}

public enum TranscriptDecision: Equatable {
    case none
    case scroll(target: String, animated: Bool)
    case pill(TranscriptPill)
}

/// Following the newest message, told apart from the owner's own scrolling: content that grows while the offset stays
/// is growth (a streaming reply, a row filling), and keeps the view pinned to the bottom while it follows; an offset
/// that changes without growth is the owner scrolling. FERB's own scrolls are announced (`scrolledByFERB`) and never
/// count as the owner's.
public struct TranscriptFollow: Equatable {
    /// The owner scrolled within this long: an arrival shows a pill instead of moving the view (D12).
    public static let recentScroll: TimeInterval = 10
    /// Within this of the bottom counts as at the bottom.
    public static let bottomSlack: CGFloat = 28

    public private(set) var following = true
    public private(set) var ownerScrolledAt: Date?
    private var offset: CGFloat?
    private var height: CGFloat?
    private var settlingUntil: Date?

    public init() {}

    /// A new measurement: `offset` is how far the content's top is above the view's top, `contentHeight` its height.
    /// Returns true when the view should be pinned to the bottom (it grew while following).
    @discardableResult
    public mutating func observe(offset: CGFloat, contentHeight: CGFloat, viewport: CGFloat, at now: Date) -> Bool {
        defer { self.offset = offset; self.height = contentHeight }
        guard let lastOffset = self.offset, let lastHeight = self.height else { return false }
        let moved = abs(offset - lastOffset) > 0.5
        let grew = abs(contentHeight - lastHeight) > 0.5
        let atBottom = offset + viewport >= contentHeight - Self.bottomSlack
        if let until = settlingUntil, now < until {
            if moved { following = atBottom }
            return false
        }
        settlingUntil = nil
        if moved && !grew {
            ownerScrolledAt = now
            following = atBottom
            return false
        }
        if grew && !moved { return following && !atBottom }
        if grew && moved { following = following || atBottom }
        return false
    }

    /// FERB is about to scroll (to the bottom, or to an item): offset changes until it settles aren't the owner's.
    public mutating func scrolledByFERB(at now: Date, settle: TimeInterval = 0.6) {
        settlingUntil = now.addingTimeInterval(settle)
    }

    /// The owner tapped the jump control or a pill: from here the view follows again.
    public mutating func follow() { following = true }

    public func ownerScrolledRecently(at now: Date) -> Bool {
        guard let ownerScrolledAt else { return false }
        return now.timeIntervalSince(ownerScrolledAt) < Self.recentScroll
    }

    /// What an arrival does. `target`: the item's top and bottom in the view's coordinates (0 is the view's top), nil
    /// when it isn't measured yet. A question, an answer or a task's end brings the item's top into view (animated,
    /// except under Reduce Motion or when the notch is just opening for it), unless the owner scrolled within the
    /// last 10 s: then a pill names it. A quiet fill never moves the view: a pill when it is out of sight.
    public func decide(_ request: TranscriptScrollRequest, target: ClosedRange<CGFloat>?, viewport: CGFloat,
                       reduceMotion: Bool, at now: Date) -> TranscriptDecision {
        let topVisible = target.map { $0.lowerBound >= -1 && $0.lowerBound < viewport - 8 } ?? false
        let wholeVisible = target.map { $0.lowerBound >= -1 && $0.upperBound <= viewport + 1 } ?? false
        let pill = TranscriptPill(reason: request.reason, target: request.target, label: request.label, count: 1,
                                  up: (target?.lowerBound ?? 0) < 0)
        switch request.reason {
        case .quietFill:
            return topVisible ? .none : .pill(pill)
        case .collapse:
            return topVisible ? .none : .scroll(target: request.target, animated: !reduceMotion)
        case .answer, .taskDone, .needsYou:
            if request.revealing { return .scroll(target: request.target, animated: false) }
            if ownerScrolledRecently(at: now) { return topVisible ? .none : .pill(pill) }
            if wholeVisible { return .none }
            return .scroll(target: request.target, animated: !reduceMotion)
        }
    }
}
