import SwiftUI

// The design book's numbers that the lab keeps in ~/Desktop/ferb-glass-lab tokens.json (at LabSource.commit) rather
// than in its Swift: the radius per role, the spacing steps and the Mac's type ramp. The phone screens draw their own
// numbers (Mobile.swift, MobileScreens.swift); these are here so the two phones and the book can be checked against
// each other (LabNumbersTests reads Android's copy of the lab's numbers).

/// tokens.json `radius`: a radius belongs to a role.
public enum Radius {
    public static let chip: CGFloat = 6
    public static let key: CGFloat = 5.5
    public static let buttonCompact: CGFloat = 7
    public static let button: CGFloat = 9
    public static let list: CGFloat = 12
    public static let card: CGFloat = 14
    public static let bubble: CGFloat = 16
    public static let window: CGFloat = 20
    public static let notchBottom: CGFloat = 26
    public static let phoneSheet: CGFloat = 34

    public static let all: [String: CGFloat] = [
        "chip": chip, "key": key, "buttonCompact": buttonCompact, "button": button, "list": list, "card": card,
        "bubble": bubble, "window": window, "notchBottom": notchBottom, "phoneSheet": phoneSheet,
    ]
}

/// tokens.json `space`.
public enum Space {
    public static let xs: CGFloat = 4
    public static let s: CGFloat = 8
    public static let m: CGFloat = 12
    public static let l: CGFloat = 16
    public static let xl: CGFloat = 24
    public static let all: [CGFloat] = [xs, s, m, l, xl]
}

/// tokens.json `type`: the Mac's ramp, which Theme.swift's `Text.ink` draws (tightened by 0.3 from 17 up).
public enum MacType {
    public struct Role: Equatable {
        public let size: CGFloat
        public let weight: String
        public let tracking: CGFloat
    }
    public static let title = Role(size: 22, weight: "semibold", tracking: inkTracking(22))
    public static let heading = Role(size: 17, weight: "semibold", tracking: inkTracking(17))
    public static let body = Role(size: 13, weight: "regular", tracking: inkTracking(13))
    public static let rowTitle = Role(size: 13, weight: "medium", tracking: inkTracking(13))
    public static let label = Role(size: 12.5, weight: "medium", tracking: inkTracking(12.5))
    public static let note = Role(size: 12, weight: "regular", tracking: inkTracking(12))
    public static let all: [String: Role] = [
        "title": title, "heading": heading, "body": body, "rowTitle": rowTitle, "label": label, "note": note,
    ]

    /// Theme.swift `Text.ink`: `size >= 17 ? -0.3 : 0`.
    public static func inkTracking(_ size: CGFloat) -> CGFloat { size >= 17 ? -0.3 : 0 }
}
