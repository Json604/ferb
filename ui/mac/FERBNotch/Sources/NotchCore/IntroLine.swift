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
    public init(text: String, detail: String? = nil, choices: [IntroChoice] = [], entry: IntroEntry? = nil,
                iconPath: String? = nil) {
        self.text = text
        self.detail = detail
        self.choices = choices
        self.entry = entry
        self.iconPath = iconPath
    }
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
    public let detail: String
    public let complete: Bool
    public var actionID: String?
    public var active: Bool
    public var status: String?
    public init(id: String, title: String, detail: String, complete: Bool,
                actionID: String? = nil, active: Bool = false, status: String? = nil) {
        self.id = id; self.title = title; self.detail = detail; self.complete = complete
        self.actionID = actionID; self.active = active; self.status = status
    }
}
