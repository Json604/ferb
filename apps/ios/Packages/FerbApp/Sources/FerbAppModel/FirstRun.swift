import Foundation

/// The first run's asks (owner kb PHN-635; PHN-636..639; Android's FirstRun): once, after the first unlock, FERB asks
/// the phone for what it needs, from one page and one Continue: notifications, the city (when-in-use location, an
/// approximate one; never always), the camera (the pairing code moments later) and the microphone with speech
/// recognition (hold to talk). An ask iOS already decided, or one already answered or passed over, is never on the
/// page; an update that adds an ask shows only it. Pure: what the phone decided and what was asked come in.
public struct FirstRun {
    public enum Ask: String, CaseIterable, Sendable { case notifications, location, camera, microphone }

    let wanted: [Ask]
    let decided: (Ask) -> Bool
    let asked: () -> Set<String>
    let record: (Set<String>) -> Void

    public init(wanted: [Ask] = Ask.allCases, decided: @escaping (Ask) -> Bool, asked: @escaping () -> Set<String>,
                record: @escaping (Set<String>) -> Void) {
        self.wanted = wanted
        self.decided = decided
        self.asked = asked
        self.record = record
    }

    /// What the page asks for now, in order; empty: no page.
    public func pending() -> [Ask] {
        let done = asked()
        return wanted.filter { !done.contains($0.rawValue) && !decided($0) }
    }

    /// The page shows over the pairing page or the app itself, never before the unlock (PHN-835).
    public func showsOver(unlockedPage: Bool) -> Bool { unlockedPage && !pending().isEmpty }

    /// iOS's question for `ask` was answered (either way): it is never asked again.
    public func answered(_ ask: Ask) { record(asked().union([ask.rawValue])) }

    /// Not now: the page's asks are passed over; each feature still explains itself when it is used.
    public func notNow() { record(asked().union(pending().map(\.rawValue))) }
}
