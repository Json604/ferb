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

struct FeedbackSettingsView: View {
    @ObservedObject var feedback: SoundFeedback

    var body: some View {
        Form {
            Section("Sound") {
                Toggle("Play feedback sounds", isOn: $feedback.enabled)
                HStack {
                    Image(systemName: "speaker.fill")
                    Slider(value: $feedback.volume, in: 0...0.5) { Text("Feedback volume") }
                    Image(systemName: "speaker.wave.2.fill")
                }
                .disabled(!feedback.enabled)
                Button("Play completion sound") { feedback.play(.completed) }
                    .disabled(!feedback.enabled)
            }
            Section("Shortcuts") {
                LabeledContent("Open / close FERB", value: "Double-tap right Option")
                LabeledContent("Dictation", value: "Hold right Command + Option")
                LabeledContent("Submit / new line", value: "Return / Shift–Return")
            }
            Section {
                Text("Tasks connect to your local FERB daemon. Dictation uses your microphone while recording; release the shortcut to review the text.")
                    .font(.callout).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .frame(width: 410, height: 350)
    }
}
