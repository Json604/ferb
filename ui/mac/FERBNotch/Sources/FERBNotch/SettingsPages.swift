import AppKit
import SwiftUI
import NotchCore

enum SettingsTab: Hashable, CaseIterable, Identifiable {
    case general, ai, permissions, browsers, notifications, privacy, advanced

    var id: SettingsTab { self }

    var title: String {
        switch self {
        case .general: return "General"
        case .ai: return "AI"
        case .permissions: return "Permissions"
        case .browsers: return "Browsers"
        case .notifications: return "Notifications"
        case .privacy: return "Privacy"
        case .advanced: return "Advanced"
        }
    }
}

struct SettingsView: View {
    @ObservedObject var store: FERBWindowStore
    @ObservedObject var selection: FERBWindowSelection
    let actions: FERBAppActions

    var body: some View {
        TabView(selection: $selection.settingsTab) {
            GeneralSettingsPage(feedback: actions.feedback, cursor: actions.cursor)
                .tabItem { Text(SettingsTab.general.title) }.tag(SettingsTab.general)
        }
    }
}

/// Settings → General: the notch's shortcuts, FERB's pointer and sounds (moved here from the old sound window). Plans
/// F and G add "Startup" (open at login) and "Updates".
struct GeneralSettingsPage: View {
    @ObservedObject var feedback: SoundFeedback
    let cursor: CursorController
    @AppStorage("cursor.enabled") private var cursorEnabled = true

    var body: some View {
        Form {
            Section("Notch") {
                LabeledContent("Open / close FERB", value: "Double-tap right Option, or Control–Option–Space")
                LabeledContent("Dictation", value: "Hold right Command + Option")
                LabeledContent("Submit / new line", value: "Return / Shift–Return")
            }
            Section("FERB's pointer") {
                Toggle("Show FERB's pointer when it acts on screen", isOn: $cursorEnabled)
                    .onChange(of: cursorEnabled) { _, on in cursor.enabled = on }
            }
            Section("Sounds") {
                Toggle("Play feedback sounds", isOn: $feedback.enabled)
                Slider(value: $feedback.volume, in: 0...0.5) { Text("Volume") }.disabled(!feedback.enabled)
                Button("Play completion sound") { feedback.play(.completed) }.disabled(!feedback.enabled)
            }
        }
        .formStyle(.grouped)
    }
}
