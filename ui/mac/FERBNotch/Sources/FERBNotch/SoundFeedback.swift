import AppKit
import SwiftUI

@MainActor
final class SoundFeedback: ObservableObject {
    enum Cue: String {
        case accepted = "Tink"
        case listening = "Pop"
        case finishedListening = "Bottle"
        case completed = "Glass"
        case attention = "Basso"
    }

    @Published var enabled: Bool {
        didSet { UserDefaults.standard.set(enabled, forKey: "feedback.enabled") }
    }
    @Published var volume: Double {
        didSet { UserDefaults.standard.set(volume, forKey: "feedback.volume") }
    }
    @Published var magneticEnabled: Bool {
        didSet { UserDefaults.standard.set(magneticEnabled, forKey: "feedback.magneticEnabled") }
    }
    private var currentSound: NSSound?

    init() {
        let defaults = UserDefaults.standard
        magneticEnabled = defaults.object(forKey: "feedback.magneticEnabled") as? Bool ?? true
        enabled = defaults.object(forKey: "feedback.enabled") as? Bool ?? true
        volume = defaults.object(forKey: "feedback.volume") as? Double ?? 0.18
    }

    var playbackVolume: Float { enabled ? Float(min(1, max(0, volume))) : 0 }

    func play(_ cue: Cue) {
        guard playbackVolume > 0 else { return }
        currentSound?.stop()
        let candidate = NSSound(named: NSSound.Name(cue.rawValue))?.copy() as? NSSound
        guard let sound = candidate else { return }
        sound.volume = playbackVolume
        currentSound = sound
        sound.play()
    }

}
