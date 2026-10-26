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
            GeneralSettingsPage(feedback: actions.feedback, cursor: actions.cursor, loginItems: actions.loginItems, updates: actions.updates)
                .tabItem { Text(SettingsTab.general.title) }.tag(SettingsTab.general)
            AISettingsPage(store: store, actions: actions)
                .tabItem { Text(SettingsTab.ai.title) }.tag(SettingsTab.ai)
            PermissionsSettingsPage(actions: actions)
                .tabItem { Text(SettingsTab.permissions.title) }.tag(SettingsTab.permissions)
            BrowsersSettingsPage(actions: actions)
                .tabItem { Text(SettingsTab.browsers.title) }.tag(SettingsTab.browsers)
            NotificationsSettingsPage(store: store)
                .tabItem { Text(SettingsTab.notifications.title) }.tag(SettingsTab.notifications)
            PrivacySettingsPage(store: store)
                .tabItem { Text(SettingsTab.privacy.title) }.tag(SettingsTab.privacy)
            AdvancedSettingsPage(store: store, actions: actions)
                .tabItem { Text(SettingsTab.advanced.title) }.tag(SettingsTab.advanced)
        }
    }
}

/// Settings → General: the notch's shortcuts, FERB's pointer and sounds (moved here from the old sound window). Plans
/// F and G add "Startup" (open at login) and "Updates".
struct GeneralSettingsPage: View {
    @ObservedObject var feedback: SoundFeedback
    let cursor: CursorController
    var loginItems: LoginItemController? = nil
    var updates: UpdateCoordinator? = nil
    @AppStorage("cursor.enabled") private var cursorEnabled = true

    var body: some View {
        Form {
            if let loginItems {
                Section("Startup") { LoginItemRow(loginItems: loginItems) }
            }
            if let updates {
                Section("Updates") { UpdatesSection(updates: updates) }
            }
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

/// Output of FERB.app's own `ferb` commands run from Settings (plan C's runner): each line as it arrives, then the exit
/// status. Uninstall runs plan F's hook first.
@MainActor
final class FerbOutput: ObservableObject {
    @Published private(set) var lines: [String] = []
    @Published private(set) var status: Int32?
    @Published private(set) var running = false

    func run(_ arguments: [String], with actions: FERBAppActions, then: (() -> Void)? = nil) {
        guard let runFerb = actions.runFerb else { return }
        lines = []
        status = nil
        running = true
        let started = runFerb(arguments, { [weak self] line in self?.lines.append(line) },
                              { [weak self] code in
                                  self?.status = code
                                  self?.running = false
                                  then?()
                              })
        if !started {
            running = false
            lines = ["FERB is already running a setup step; try again when it finishes."]
        }
    }

    func uninstall(with actions: FERBAppActions) {
        actions.beforeUninstall?()
        run(["uninstall"], with: actions)
    }
}

private struct OutputLines: View {
    @ObservedObject var output: FerbOutput
    var body: some View {
        ForEach(Array(output.lines.enumerated()), id: \.offset) { _, line in
            Text(line).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
        }
    }
}

private struct OnlyInTheApp: View {
    var body: some View {
        Text("This is set in the installed FERB.app.").foregroundStyle(.secondary)
    }
}

/// Settings → AI: plan C's Choose your AI (sign in with ChatGPT, use Claude, an API key into the Keychain), whether the
/// service answers, and signing FERB's own Codex out. This page never shows or sends a key (rule 14).
struct AISettingsPage: View {
    @ObservedObject var store: FERBWindowStore
    let actions: FERBAppActions
    @StateObject private var output = FerbOutput()

    var body: some View {
        if let setup = actions.setup {
            Form {
                Section { AIStepView(model: setup) }
                Section("Check") {
                    Button("Check connection") { Task { await store.checkRuntime(setup.ai?.current ?? "codex") } }
                    Button("Sign out of ChatGPT") {
                        output.run(["setup", "--codex-logout"], with: actions) { Task { await setup.refreshAI() } }
                    }
                    .disabled(output.running)
                    OutputLines(output: output)
                }
            }
            .formStyle(.grouped)
            .task { await setup.refreshAI() }
        } else {
            Form { OnlyInTheApp() }.formStyle(.grouped)
        }
    }
}

/// Settings → Permissions: plan C's rows, each as FERB's background service (or FERB.app) itself sees it, with Grant
/// and the System Settings pane; it re-reads while shown.
struct PermissionsSettingsPage: View {
    let actions: FERBAppActions

    var body: some View {
        if let setup = actions.setup {
            Form { PermissionsStepView(model: setup) }
                .formStyle(.grouped)
                .onAppear { setup.startPolling() }
                .onDisappear { setup.stopPolling() }
        } else {
            Form { OnlyInTheApp() }.formStyle(.grouped)
        }
    }
}

/// Settings → Browsers: plan C's browsers and profiles (not installed / installed / on / paired) and Add to….
struct BrowsersSettingsPage: View {
    let actions: FERBAppActions

    var body: some View {
        if let setup = actions.setup {
            Form { BrowsersStepView(model: setup) }
                .formStyle(.grouped)
                .task { await setup.refreshBrowsers() }
        } else {
            Form { OnlyInTheApp() }.formStyle(.grouped)
        }
    }
}

/// Settings → Notifications: which kinds FERB posts while the notch is closed (plan D's categories).
struct NotificationsSettingsPage: View {
    @ObservedObject var store: FERBWindowStore

    var body: some View {
        Form {
            Section("Notify me about") {
                ForEach(BannerCategory.allCases, id: \.rawValue) { category in
                    Toggle(category.title, isOn: store.banner(category))
                }
            }
            Section {
                if let url = URL(string: "x-apple.systempreferences:com.apple.preference.notifications") {
                    Button("Open macOS notification settings") { NSWorkspace.shared.open(url) }
                }
            }
        }
        .formStyle(.grouped)
    }
}

/// Settings → Privacy: what FERB records, the apps and sites it ignores, how long it keeps records, Pause recording.
struct PrivacySettingsPage: View {
    @ObservedObject var store: FERBWindowStore
    @State private var newApp = ""
    @State private var newSite = ""

    var body: some View {
        Form {
            Section("Recording") {
                Toggle("Pause recording what happens on this Mac",
                       isOn: Binding(get: { store.recordingPaused }, set: { on in Task { await store.pauseRecording(on) } }))
                Toggle("Record window titles", isOn: store.toggle("sense.window_titles"))
                LabeledContent("Recording now", value: store.watchers.isEmpty ? "nothing" : store.watchers.joined(separator: ", "))
            }
            Section("Apps FERB ignores") {
                StringListEditor(items: store.settings.strings("privacy.exclude_apps"),
                                 placeholder: "Bundle id or app name, e.g. com.1password.1password",
                                 newItem: $newApp) { list in Task { await store.set("privacy.exclude_apps", .strings(list)) } }
            }
            Section("Sites FERB ignores") {
                StringListEditor(items: store.settings.strings("privacy.exclude_domains"),
                                 placeholder: "Site, e.g. bank.example.com",
                                 newItem: $newSite) { list in Task { await store.set("privacy.exclude_domains", .strings(list)) } }
            }
            Section("How long FERB keeps records") {
                DaysStepper(title: "Activity trail", key: "sense.trail_retention_days", store: store)
                DaysStepper(title: "Task traces", key: "retention.trace_days", store: store)
                DaysStepper(title: "Jev decisions", key: "retention.decision_days", store: store)
                DaysStepper(title: "Screenshots and evidence", key: "retention.evidence_days", store: store)
                DaysStepper(title: "Recovery copies", key: "retention.recovery_days", store: store)
                DaysStepper(title: "Resource log", key: "retention.resource_days", store: store)
            }
        }
        .formStyle(.grouped)
    }
}

struct StringListEditor: View {
    let items: [String]
    let placeholder: String
    @Binding var newItem: String
    let save: ([String]) -> Void

    var body: some View {
        ForEach(items, id: \.self) { item in
            HStack {
                Text(item).textSelection(.enabled)
                Spacer()
                Button("Remove") { save(items.filter { $0 != item }) }
            }
        }
        HStack {
            TextField(placeholder, text: $newItem).onSubmit(add)
            Button("Add", action: add)
        }
    }

    private func add() {
        let text = newItem.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        save(items + [text])
        newItem = ""
    }
}

struct DaysStepper: View {
    let title: String
    let key: String
    @ObservedObject var store: FERBWindowStore

    var body: some View {
        let days = store.settings.int(key)
        Stepper("\(title): \(days) day\(days == 1 ? "" : "s")",
                value: Binding(get: { days }, set: { value in Task { await store.set(key, .int(value)) } }),
                in: 1...Int.max)
    }
}

/// Settings → Advanced: logs, a health check, the diagnostic file, the command-line tool, uninstall.
struct AdvancedSettingsPage: View {
    @ObservedObject var store: FERBWindowStore
    let actions: FERBAppActions
    @StateObject private var output = FerbOutput()
    @State private var confirmUninstall = false

    private var logs: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".ferb/logs", isDirectory: true)
    }

    var body: some View {
        Form {
            Section("Logs") {
                Button("Open FERB's logs folder") { NSWorkspace.shared.open(logs) }
            }
            Section("Health") {
                Button("Check FERB") { Task { await store.checkHealth() } }
                ForEach(store.health) { line in LabeledContent(line.name, value: line.ok ? "OK" : line.detail) }
            }
            Section("Diagnostic file") {
                Button("Export a diagnostic file…") {
                    Task {
                        if let url = await store.exportDiagnostic(appVersion: actions.appVersion) {
                            NSWorkspace.shared.activateFileViewerSelecting([url])
                        }
                    }
                }
                Button("Send diagnostics to developer…") {
                    Task { await store.sendDiagnostic(appVersion: actions.appVersion) }
                }
                if let line = store.diagnosticLine { Text(line).textSelection(.enabled) }
                Text("Written on this Mac with secrets removed. Send opens a new email in your Mail app with the file "
                     + "attached; nothing goes until you press Send there.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            if actions.runFerb != nil {
                Section("Command-line tool and uninstall") {
                    Button("Install command-line tool") { output.run(["setup", "--cli"], with: actions) }
                        .disabled(output.running)
                    Button("Uninstall FERB…") { confirmUninstall = true }.disabled(output.running)
                    OutputLines(output: output)
                }
            }
        }
        .formStyle(.grouped)
        .alert("Uninstall FERB?", isPresented: $confirmUninstall) {
            Button("Uninstall", role: .destructive) { output.uninstall(with: actions) }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("FERB stops its background service and removes its browser connection, its login item and the `ferb` " +
                 "link. Your data in ~/.ferb stays. Then quit FERB and drag it to the Trash.")
        }
    }
}
