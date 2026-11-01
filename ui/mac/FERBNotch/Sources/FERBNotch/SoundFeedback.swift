import AppKit
import NotchCore
import SwiftUI

/// FERB's everyday sounds (notes/sound.md, owner 2026-11-01): each event names its sound from the decided family and
/// it plays through the app's one FerbSoundEngine (`player`, shared with the intro), which applies the system's UI
/// sounds setting, the master switch, each sound's own switch, Standard/Subtle and the call rule to every sound.
/// Settings → General binds the switches here. Every notification sounds once: an item shown both as a banner and in
/// the notch is carried by the banner (NotificationPoster asks `bannerSounds`), keyed on its id.
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
    /// How long a notice waits for its banner before the notch plays it: past the poster's settle (1 s).
    var noticeWait: Duration = .milliseconds(1500)
    private let defaults: UserDefaults
    /// Which side sounded each recent item (by key): a banner, the notch, or no banner will (declined or silent).
    private var sounded: [String: (by: Carrier, at: Date)] = [:]
    private var waiting: [String: Task<Void, Never>] = [:]
    private enum Carrier { case banner, notch, noBanner }
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

    /// A sound for an item a banner may also show (a task's end, a question): it plays now, and that item's banner then
    /// posts silently; unless its banner already carried the sound.
    func play(_ s: FerbSound, for key: String) {
        forget()
        guard sounded[key]?.by != .banner else { return }
        sounded[key] = (.notch, Date())
        play(s)
    }

    /// A notification in the notch chat (a notice, a scheduled task's result): one notification sound, carried by its
    /// banner when it has one. While FERB.app posts banners, the notch waits briefly for the item's banner.
    func notification(for key: String) {
        forget()
        switch sounded[key]?.by {
        case .banner?, .notch?: return
        case .noBanner?: soundInNotch(key)
        case nil:
            guard postingBanners else { return soundInNotch(key) }
            waiting[key]?.cancel()
            waiting[key] = Task { [weak self, noticeWait] in
                do { try await Task.sleep(for: noticeWait) } catch { return }
                guard !Task.isCancelled else { return }
                self?.soundInNotch(key)
            }
        }
    }

    /// A banner is being posted: whether it carries FERB's notification sound. Not when the notch already sounded for
    /// the item, nor when the sound wouldn't play now (switched off, sound off, a call).
    func bannerSounds(_ keys: [String]) -> Bool {
        forget()
        guard !keys.contains(where: { sounded[$0]?.by == .notch }) else { return false }
        guard player?.allows(.notification) == true else {
            bannerDeclined(keys)
            return false
        }
        for key in keys {
            sounded[key] = (.banner, Date())
            waiting.removeValue(forKey: key)?.cancel()
        }
        return true
    }

    /// A banner that won't sound (not posted while the notch shows its item, or posted silently): the notch sounds.
    func bannerDeclined(_ keys: [String]) {
        for key in keys where sounded[key] == nil {
            sounded[key] = (.noBanner, Date())
            if waiting[key] != nil { soundInNotch(key) }
        }
    }

    private func soundInNotch(_ key: String) {
        waiting.removeValue(forKey: key)?.cancel()
        sounded[key] = (.notch, Date())
        play(.notification)
    }

    /// Items older than the race between a banner and its notch twin are forgotten.
    private func forget() {
        let cutoff = Date().addingTimeInterval(-Self.memory)
        sounded = sounded.filter { $0.value.at > cutoff }
    }
}
