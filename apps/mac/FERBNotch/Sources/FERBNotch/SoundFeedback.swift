import AppKit
import NotchCore
import SwiftUI

/// FERB's everyday sounds (kb area app, owner 2026-11-01): each event names its sound from the decided family and
/// it plays through the app's one FerbSoundEngine (`player`, shared with the intro), which applies the system's UI
/// sounds setting, the master switch, each sound's own switch, Standard/Subtle and the call rule to every sound.
/// Settings → Sounds & Notifications binds the switches here. Every notification sounds once: an item shown both as a banner and in
/// the notch is carried by the banner (NotificationPoster asks `bannerSounds`), keyed on its id. Needs you and
/// error always play in the app; their banners post silently.
@MainActor
final class SoundFeedback: ObservableObject {
    @Published var enabled: Bool {
        didSet { defaults.set(enabled, forKey: "feedback.enabled") }
    }
    @Published var volume: Double {
        didSet { defaults.set(volume, forKey: "feedback.volume") }
    }
    @Published var subtle: Bool {
        didSet { defaults.set(subtle ? "subtle" : "standard", forKey: SoundPolicy.intensityKey) }
    }
    @Published private var switches: [SoundSwitch: Bool]
    /// The app's engine, attached at launch (AppDelegate). Without one (a test, before launch) nothing plays.
    var player: FerbSoundPlaying?
    /// FERB.app holds the daemon's banners (NotificationPoster's subscription is open): a notice may get one.
    var postingBanners = false
    /// How long a notification the daemon also banners waits for its banner: past the poster's settle (1 s).
    var noticeWait: Duration = .milliseconds(1500)
    /// How that wait passes (tests end it themselves instead of racing a clock).
    var sleep: @MainActor (Duration) async throws -> Void = { try await Task.sleep(for: $0) }
    private let defaults: UserDefaults
    /// Which side sounded each recent item (by key): a banner, the app (with which sound), or no banner will.
    private var sounded: [String: (by: Carrier, at: Date)] = [:]
    private var waiting: [String: Task<Void, Never>] = [:]
    private enum Carrier: Equatable { case banner, app(FerbSound), noBanner }
    private static let memory: TimeInterval = 30

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        enabled = defaults.object(forKey: "feedback.enabled") as? Bool ?? true
        volume = defaults.object(forKey: "feedback.volume") as? Double ?? FerbSoundEngine.defaultVolume
        subtle = defaults.string(forKey: SoundPolicy.intensityKey) == "subtle"
        switches = Dictionary(uniqueKeysWithValues: SoundSwitch.allCases.map {
            ($0, defaults.object(forKey: $0.key) as? Bool ?? $0.onByDefault)
        })
        defaults.removeObject(forKey: "feedback.magneticEnabled")     // the magnetic docking sound is gone
    }

    func isOn(_ s: SoundSwitch) -> Bool { switches[s] ?? s.onByDefault }

    func set(_ s: SoundSwitch, on: Bool) {
        switches[s] = on
        defaults.set(on, forKey: s.key)
    }

    func binding(_ s: SoundSwitch) -> Binding<Bool> {
        Binding(get: { [weak self] in self?.isOn(s) ?? s.onByDefault }, set: { [weak self] in self?.set(s, on: $0) })
    }

    func play(_ s: FerbSound) { player?.play(s, at: nil) }

    /// Settings' Play: the switch's sound, as the engine would play it now (a switch that's off stays silent).
    func preview(_ s: SoundSwitch) { play(s.preview) }

    /// A sound for an item a banner may also show (a task's end, a question, a handoff): it always plays, once per item
    /// and sound, and that item's banner then posts silently. Needs you and error are never left to a banner.
    func play(_ s: FerbSound, for keys: [String]) {
        forget()
        guard !keys.contains(where: { sounded[$0]?.by == .app(s) }) else { return }
        for key in keys { sounded[key] = (.app(s), Date()) }
        play(s)
    }

    func play(_ s: FerbSound, for key: String) { play(s, for: [key]) }

    /// Notifications in the notch chat (a batch of notices, a scheduled task's result): one notification sound for all
    /// of `keys`, carried by a banner when one sounded for them. `bannerMayFollow`: the daemon is sending a banner of
    /// it too (a notice marked `banner`, a task's result while FERB.app posts); only then the notch waits briefly.
    func notification(for keys: [String], bannerMayFollow: Bool) {
        forget()
        guard !keys.isEmpty, !keys.contains(where: carried) else { return }
        guard bannerMayFollow, postingBanners, !keys.contains(where: { sounded[$0]?.by == .noBanner }) else {
            return soundInNotch(keys)
        }
        let wait = Task { [weak self, noticeWait, sleep] in
            do { try await sleep(noticeWait) } catch { return }
            guard !Task.isCancelled else { return }
            self?.soundInNotch(keys)
        }
        for key in keys { waiting[key] = wait }
    }

    /// A banner is being posted: whether it carries FERB's notification sound. Not when the app already sounded for the
    /// item, nor when the sound wouldn't play now (switched off, sound off, a call); then the notch sounds instead.
    func bannerSounds(_ keys: [String]) -> Bool {
        forget()
        guard !keys.contains(where: { if case .app? = sounded[$0]?.by { return true } else { return false } }) else {
            return false
        }
        guard player?.allows(.notification) == true else {
            bannerDeclined(keys)
            return false
        }
        for key in keys {
            sounded[key] = (.banner, Date())
            if let wait = waiting[key] { drop(wait) }
        }
        return true
    }

    /// A banner that won't sound (not posted while the notch shows its item, posted silently, or macOS's sound for FERB
    /// is off): the notch sounds instead, at once if it was waiting.
    func bannerDeclined(_ keys: [String]) {
        for key in keys where sounded[key] == nil {
            sounded[key] = (.noBanner, Date())
            if let wait = waiting[key] { soundInNotch(drop(wait)) }
        }
    }

    private func carried(_ key: String) -> Bool {
        switch sounded[key]?.by {
        case .banner?, .app?: return true
        case .noBanner?, nil: return false
        }
    }

    private func soundInNotch(_ keys: [String]) {
        for key in keys {
            if let wait = waiting[key] { drop(wait) }
            sounded[key] = (.app(.notification), Date())
        }
        play(.notification)
    }

    /// Cancels a wait and returns every key it was for.
    @discardableResult
    private func drop(_ wait: Task<Void, Never>) -> [String] {
        wait.cancel()
        let keys = waiting.filter { $0.value == wait }.map(\.key)
        for key in keys { waiting[key] = nil }
        return keys
    }

    /// Items older than the race between a banner and its notch twin are forgotten.
    private func forget() {
        let cutoff = Date().addingTimeInterval(-Self.memory)
        sounded = sounded.filter { $0.value.at > cutoff }
    }
}
