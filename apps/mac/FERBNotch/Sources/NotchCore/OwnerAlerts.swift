import Foundation

/// FERB's alerts (owner, 2026-11-21, APP-1105; APP-1106, APP-1107): the Mac getting hot, insomnia mode's end, a database
/// set aside, an update installed, a restart put off… are shown for a while above the composer, never kept in the chat.
/// One from the daemon stays until the owner puts it away on any device (`attention.ack`, which every other device hears
/// as `attention.acked`); FERB.app's own last for this run, until put away.
public struct OwnerAlert: Equatable, Identifiable {
    public let id: String
    public var text: String
    public let at: Date
    /// The daemon's (put away on every device); false: FERB.app's own.
    public let fromDaemon: Bool
    /// A button that goes with it (a phone asking to pair: Settings → Phone).
    public var card: ResultCard?

    public init(id: String, text: String, at: Date = Date(), fromDaemon: Bool, card: ResultCard? = nil) {
        self.id = id
        self.text = text
        self.at = at
        self.fromDaemon = fromDaemon
        self.card = card
    }
}

public enum OwnerAlerts {
    /// The daemon names every alert it makes now `alert:<id>` (ferb-core `ALERT_PREFIX`); the other families are the ids
    /// alerts had before (ferb-core `EARLIER_ALERTS`) and FERB.app's own alert lines, so what an earlier build saved in
    /// the chat is hidden too. FERB's own id formats, never words judged.
    static let families = [
        "alert:", "insomnia-", "set-aside-", "window-shown-", "refused-", "phone-unused-", "screen-until-back-",
        // FERB.app's own: "Updated to …", an update that failed or asks first, a restart put off, a phone asking to
        // pair, the "Are you back?" line (GATE-618), insomnia's end said by FERB.app
        "update-", "restart-after-tasks-", "phone-pair-", "screen-back-",
    ]

    /// Whether a notice id names one of FERB's alerts.
    public static func isAlert(id: String) -> Bool {
        families.contains { id.hasPrefix($0) }
    }

    /// Whether a daemon notice row is an alert: marked so (`alert`), or named so by its id (an older daemon's).
    public static func isAlert(row: [String: Any]) -> Bool {
        if row["alert"] as? Bool == true { return true }
        return (row["id"] as? String).map(isAlert(id:)) ?? false
    }

    /// One alert added or reworded in place (the same id), the newest last. Returns whether anything changed.
    @discardableResult
    public static func show(_ alert: OwnerAlert, in alerts: inout [OwnerAlert]) -> Bool {
        guard !alert.id.isEmpty, !alert.text.isEmpty else { return false }
        if let index = alerts.firstIndex(where: { $0.id == alert.id }) {
            guard alerts[index].text != alert.text || alerts[index].card != alert.card else { return false }
            alerts[index].text = alert.text
            alerts[index].card = alert.card
            return true
        }
        alerts.append(alert)
        return true
    }

    /// How long a toast stays in front of the owner before it leaves by itself (APP-1109): the six seconds of the
    /// planning preview (APP-624). Counted only while the chat shows, the stack is closed and the pointer is off it.
    public static let toastSeconds: Double = 6

    /// An alert that needs the owner (APP-1109): one with a button (a phone asking to pair) or an update that asks
    /// first. It never leaves by itself; he puts it away or acts.
    public static func needsOwner(_ alert: OwnerAlert) -> Bool {
        alert.card != nil || alert.id.hasPrefix("update-ask-")
    }

    /// The toast shown while the stack is closed: the newest alert.
    public static func toast(_ alerts: [OwnerAlert]) -> OwnerAlert? { alerts.last }

    /// The daemon's alerts as its status lists them now (every one not yet put away): one put away elsewhere leaves,
    /// one not shown yet comes; FERB.app's own stay.
    @discardableResult
    public static func settle(daemon ids: Set<String>, in alerts: inout [OwnerAlert]) -> Bool {
        let before = alerts.count
        alerts.removeAll { $0.fromDaemon && !ids.contains($0.id) }
        return alerts.count != before
    }
}
