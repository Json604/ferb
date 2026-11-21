import FerbCore
import FerbMobile
import Foundation

/// Insomnia mode's words that depend on the Mac's state (APP-981; Android's model/MacAsleep.kt): the warning's
/// closed-lid sentence and the helper's problem, and what the phone says when the Mac can't be reached while the mode
/// was on (it may be asleep). One place for the rule, so the notification, the chat's line and the waiting messages agree.
public enum MacAsleep {
    /// The Mac may be asleep: the link has been lost for the can't-reach time (`lost`; never just connecting), the last
    /// status said insomnia mode was on, and the link hasn't stopped for good (removed, pair again: those say their own).
    public static func shown(_ status: PhoneStatus?, _ link: LinkStatus?, lost: Bool) -> Bool {
        lost && status?.insomnia?.on == true && link?.phase != .online && link?.phase != .stopped
    }

    /// The Mac's own words while it is asleep and only briefly awake (one of macOS's Power Nap wakes; kb SENSE-430):
    /// replies are slow and screen work waits. From the last status, mode on or off, until the link stops for good.
    public static func line(_ status: PhoneStatus?, _ link: LinkStatus?) -> String? {
        guard let i = status?.insomnia, i.asleep, link?.phase != .stopped,
              let text = i.asleepLine, !text.trimmingCharacters(in: .whitespaces).isEmpty else { return nil }
        return text
    }

    /// A message waiting for the Mac waits for it to wake: the may-be-asleep rule above, or the Mac said it is asleep.
    public static func waiting(_ status: PhoneStatus?, _ link: LinkStatus?, lost: Bool) -> Bool {
        shown(status, link, lost: lost) || line(status, link) != nil
    }

    /// The warning's words: the lid-open advice, then the Mac's own closed-lid sentence (`fallback` from an older Mac).
    public static func body(open: String, fallback: String, _ i: Insomnia?) -> String {
        let lid = i?.lidLine.flatMap { $0.trimmingCharacters(in: .whitespaces).isEmpty ? nil : $0 } ?? fallback
        return "\(open) \(lid)"
    }

    /// Why FERB's lid helper isn't on, in the Mac's plain words (nil when nothing is wrong, or an older Mac).
    public static func helperProblem(_ i: Insomnia?) -> String? {
        guard let p = i?.helper?.problem, !p.trimmingCharacters(in: .whitespaces).isEmpty else { return nil }
        return p
    }

    /// What a message waiting for the Mac says (a catalog key): the phone's unlock, the Mac asleep, or "Waiting for
    /// your Mac".
    public static func waitingLine(_ waitingFor: String?, asleep: Bool) -> String {
        if waitingFor == "unlock" { return "outbox_waiting_unlock" }
        return asleep ? "outbox_mac_asleep" : "outbox_waiting"
    }
}
