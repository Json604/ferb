import Foundation

/// One hover look for everything in the chat a click acts on (APP-753, owner 2026-11-06: "they are not changing color
/// on hover - they shoudl adn also if its a link"): file cards, the sites row and its link text, its "+N" box and the
/// links inside answers. Pure values; the views draw them (CardHover, MessageTextView). Nothing opens on hover
/// (APP-715): only colour changes, and a link shows the pointing hand.
public enum HoverLook {
    /// The white fill a card or chip shows over FERB's black glass, at rest and under the pointer.
    public static func fill(hovered: Bool) -> Double { hovered ? 0.17 : 0.08 }
    /// The "+N" overflow box: a step brighter than the chip it sits in.
    public static func badgeFill(hovered: Bool) -> Double { hovered ? 0.26 : 0.14 }
    /// How far a link's colour moves towards white under the pointer (0 at rest).
    public static func linkLift(hovered: Bool) -> Double { hovered ? 0.45 : 0 }
    /// How long the change takes; nil under Reduce Motion (it changes at once).
    public static func duration(reduceMotion: Bool) -> Double? { reduceMotion ? nil : 0.12 }
    /// The overflow count as the badge says it: "+3"; none for nothing more.
    public static func overflow(_ count: Int) -> String? { count > 0 ? "+\(count)" : nil }
}
