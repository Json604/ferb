import Foundation

/// One thing FERB says in the first-run intro: a line, and an optional smaller line under it. Shown as text (in the
/// notch, or in the bubble beside the orb), never read aloud. A line in the notch may offer the owner's choices and
/// one field to type into (an API key, an address); the intro controller acts on what was picked.
public struct IntroLine: Equatable, Sendable {
    public var text: String
    public var detail: String?
    public var choices: [IntroChoice]
    public var entry: IntroEntry?
    /// An app whose icon shows beside the line (the orb's bubble pointing at an app): its bundle's path.
    public var iconPath: String?
    public var checklist: [IntroChecklistItem] = []
    /// What the checklist's description box says while no item is hovered or active (the permissions' hint when nil).
    public var checklistHint: String?
    /// Set while FERB waits on something (a sign-in, the scan, a pairing, a task): the line shows that it is alive
    /// (a spinner) and, past a few seconds, how long it has been waiting (APP-665). Nil: the line only says something.
    public var since: Date?
    public init(text: String, detail: String? = nil, choices: [IntroChoice] = [], entry: IntroEntry? = nil,
                iconPath: String? = nil) {
        self.text = text
        self.detail = detail
        self.choices = choices
        self.entry = entry
        self.iconPath = iconPath
    }

    /// This line, shown as a wait that started at `since` (APP-665).
    public func waiting(since: Date) -> IntroLine {
        var line = self
        line.since = since
        return line
    }

    /// How long a wait has run, as the line shows it ("0:07", "1:02"); nil for the first few seconds, when a spinner
    /// says enough (APP-665).
    public static func elapsed(since: Date, now: Date) -> String? {
        let seconds = Int(now.timeIntervalSince(since))
        guard seconds >= elapsedAfter else { return nil }
        return String(format: "%d:%02d", seconds / 60, seconds % 60)
    }

    /// A wait shorter than this shows only its spinner.
    public static let elapsedAfter = 3
}

/// A button under FERB's line: its id is what the intro controller is told, its title what the owner reads.
public struct IntroChoice: Equatable, Sendable {
    public let id: String
    public let title: String
    public init(_ id: String, _ title: String) {
        self.id = id
        self.title = title
    }
}

/// A field under FERB's line. `secure`: typed text is hidden and never kept by the line (an API key).
public struct IntroEntry: Equatable, Sendable {
    public let placeholder: String
    public let secure: Bool
    public init(placeholder: String, secure: Bool) {
        self.placeholder = placeholder
        self.secure = secure
    }
}

public struct IntroChecklistItem: Equatable, Sendable, Identifiable {
    public let id: String
    public let title: String
    /// Drawn after the title in quieter type, shortened in the middle first and whole on hover: a browser profile's
    /// signed-in account (HAND-424); "" for none.
    public let secondary: String
    public let detail: String
    public let complete: Bool
    public var actionID: String?
    public var active: Bool
    public var status: String?
    public init(id: String, title: String, secondary: String = "", detail: String, complete: Bool,
                actionID: String? = nil, active: Bool = false, status: String? = nil) {
        self.id = id; self.title = title; self.secondary = secondary; self.detail = detail; self.complete = complete
        self.actionID = actionID; self.active = active; self.status = status
    }
}
