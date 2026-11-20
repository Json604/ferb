import Foundation

/// The app's own settings on this phone (never FERB's data: no chat, no question words), in UserDefaults under the
/// same names as Android's Prefs.kt. Tests pass a suite of their own. There is no Stay connected on iPhone (kb
/// PHN-566): iOS gives an app in the background only a short while.
public final class Prefs: @unchecked Sendable {
    private let d: UserDefaults

    public init(defaults: UserDefaults = .standard) {
        d = defaults
    }

    /// PHN-420 on iOS (kb PHN-565): hide FERB while the screen is recorded or mirrored; on unless the owner turns it
    /// off. The app switcher is always covered.
    public var blockScreenshots: Bool {
        get { d.object(forKey: "secure") as? Bool ?? true }
        set { d.set(newValue, forKey: "secure") }
    }

    /// PHN-422: the lock screen never shows the words of FERB's own checks (on by default, a setting).
    public var hideChecksOnLockScreen: Bool {
        get { d.object(forKey: "hide_checks") as? Bool ?? true }
        set { d.set(newValue, forKey: "hide_checks") }
    }

    /// SENSE-420: the owner turned on sharing this phone's city with FERB (and iOS allowed an approximate location).
    /// Off until he does, or allows the location at the first run (kb PHN-638); the city itself is kept in memory only.
    public var shareCity: Bool {
        get { d.bool(forKey: "share_city") }
        set { d.set(newValue, forKey: "share_city") }
    }

    /// When this phone was first seen paired (epoch ms), for "Paired with … since".
    public var pairedSince: Int64 {
        get { (d.object(forKey: "paired_since") as? NSNumber)?.int64Value ?? 0 }
        set { d.set(NSNumber(value: newValue), forKey: "paired_since") }
    }

    /// The first open after pairing has not happened yet (the corpus's justPaired).
    public var justPaired: Bool {
        get { d.bool(forKey: "just_paired") }
        set { d.set(newValue, forKey: "just_paired") }
    }

    /// The previous open (epoch seconds) and its time zone: the corpus's awayHours, lastOpen, firstToday, newPlace.
    public var lastOpen: Int64 {
        get { (d.object(forKey: "last_open") as? NSNumber)?.int64Value ?? 0 }
        set { d.set(NSNumber(value: newValue), forKey: "last_open") }
    }

    public var lastZone: String? {
        get { d.string(forKey: "last_zone") }
        set { d.set(newValue, forKey: "last_zone") }
    }

    /// The first run's asks already answered or passed over (FirstRun.Ask raw values, kb PHN-636): never asked again.
    public var firstRunAsked: Set<String> {
        get { Set(d.stringArray(forKey: "first_run_asked") ?? []) }
        set { d.set(newValue.sorted(), forKey: "first_run_asked") }
    }

    /// The owner was told once, in context, before iOS's notification question.
    public var notificationsExplained: Bool {
        get { d.bool(forKey: "notify_explained") }
        set { d.set(newValue, forKey: "notify_explained") }
    }

    /// Speaking to FERB (kb PHN-604): the kinds of speech recognition (SpeechKind raw values) the owner was told about
    /// before his first use of each: on the phone itself, or Apple's speech service.
    public var voiceTold: Set<String> {
        get { Set(d.stringArray(forKey: "voice_told") ?? []) }
        set { d.set(Array(newValue).sorted(), forKey: "voice_told") }
    }

    /// Answers, results and questions already announced (entry or question ids only: never their words).
    public var announced: Set<String> {
        get { Set(d.stringArray(forKey: "announced") ?? []) }
        set { d.set(Array(newValue).sorted(), forKey: "announced") }
    }

    /// Finished tasks the owner has had the app open since (entry ids only): the home screen's "while you were away".
    public var seen: Set<String> {
        get { Set(d.stringArray(forKey: "seen") ?? []) }
        set { d.set(Array(newValue).sorted(), forKey: "seen") }
    }

    /// Forget this Mac: what belonged to that pairing goes; the phone's own switches stay.
    /// Insomnia mode's ends already said (their ids only, APP-980): each is said once, in the app or as a quiet
    /// notification, whichever sees it first. True for the first claim of `id`.
    public func claimInsomniaEnd(_ id: String) -> Bool {
        var said = Set(d.stringArray(forKey: "insomnia_said") ?? [])
        if said.contains(id) { return false }
        // an end is kept by the Mac 10 minutes: a short list is enough
        if said.count >= 32 { said = [] }
        said.insert(id)
        d.set(Array(said).sorted(), forKey: "insomnia_said")
        return true
    }

    public func forgetPairing() {
        for k in ["paired_since", "just_paired", "announced", "seen", "insomnia_said"] { d.removeObject(forKey: k) }
    }
}
