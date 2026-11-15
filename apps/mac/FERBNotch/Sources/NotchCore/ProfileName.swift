import Foundation

/// How FERB.app names a browser profile (HAND-424, APP-736): its browser, its name in quotes and the account the
/// profile is signed in to its browser with — `Google Chrome “Work” · work@example.com`; nothing after the name when it
/// isn't signed in (or FERB can't read it without Full Disk Access). Sentences use `text`; lists draw `title` and
/// `email` as two texts so only the email shortens (in the middle, keeping its domain) and show it whole on hover.
public struct ProfileName: Equatable, Sendable {
    /// The browser as the owner knows it ("Google Chrome"); "" leaves it out.
    public let label: String
    public let name: String
    /// The signed-in account, as the daemon read it now; "" when none.
    public let email: String

    public init(label: String = "", name: String, email: String = "") {
        self.label = label
        self.name = name
        self.email = email
    }

    /// The browser and the name: `Google Chrome “Work”`, or `“Work”` with no browser.
    public var title: String { label.isEmpty ? "“\(name)”" : "\(label) “\(name)”" }
    /// The name alone, quoted, with its account: `“Work” · work@example.com`.
    public var quoted: String { Self.join("“\(name)”", email) }
    /// The whole name for a sentence: `Google Chrome “Work” · work@example.com`.
    public var text: String { Self.join(title, email) }

    /// `head · email`, or `head` when there is no email.
    public static func join(_ head: String, _ email: String) -> String { email.isEmpty ? head : "\(head) · \(email)" }
}
