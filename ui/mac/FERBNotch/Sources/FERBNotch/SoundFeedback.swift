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
    private var currentSound: NSSound?

    init() {
        let defaults = UserDefaults.standard
        enabled = defaults.object(forKey: "feedback.enabled") as? Bool ?? true
        volume = defaults.object(forKey: "feedback.volume") as? Double ?? 0.18
    }

    func play(_ cue: Cue) {
        guard enabled, volume > 0 else { return }
        currentSound?.stop()
        guard let sound = NSSound(named: NSSound.Name(cue.rawValue))?.copy() as? NSSound else { return }
        sound.volume = Float(min(1, max(0, volume)))
        currentSound = sound
        sound.play()
    }
}
