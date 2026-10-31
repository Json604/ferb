import AppKit
import SwiftUI
import NotchCore

enum SettingsTab: Hashable, CaseIterable, Identifiable {
    case general, ai, permissions, browsers, notifications, privacy, data, advanced, about

    var id: SettingsTab { self }

    var title: String {
        switch self {
        case .general: return "General"
        case .ai: return "AI"
        case .permissions: return "Permissions"
        case .browsers: return "Browsers"
        case .notifications: return "Notifications"
        case .privacy: return "Privacy"
        case .data: return "Data"
        case .advanced: return "Advanced"
        case .about: return "About"
        }
    }

    var symbol: String {
        switch self {
        case .general: return "gearshape"
        case .ai: return "sparkles"
        case .permissions: return "lock.shield"
        case .browsers: return "globe"
        case .notifications: return "bell"
        case .privacy: return "hand.raised"
        case .data: return "externaldrive"
        case .advanced: return "slider.horizontal.3"
        case .about: return "info.circle"
        }
    }
}

struct SettingsView: View {
    @ObservedObject var store: FERBWindowStore
    @ObservedObject var selection: FERBWindowSelection
    let actions: FERBAppActions
    // Keep commands and drafts alive when their category leaves the detail column.
    @StateObject private var aiOutput = FerbOutput()
    @StateObject private var advancedOutput = FerbOutput()
    @StateObject private var privacyDraft = PrivacySettingsDraft()
    @StateObject private var cleanup = CleanupRunner()

    var body: some View {
        Group {
            switch selection.settingsTab {
            case .general:
                GeneralSettingsPage(feedback: actions.feedback, cursor: actions.cursor, loginItems: actions.loginItems,
                                    updates: actions.updates, onHoverEnabledChange: actions.setHoverEnabled)
            case .ai: AISettingsPage(store: store, actions: actions, output: aiOutput)
            case .permissions: PermissionsSettingsPage(actions: actions)
            case .browsers: BrowsersSettingsPage(actions: actions)
            case .notifications: NotificationsSettingsPage(store: store)
            case .privacy: PrivacySettingsPage(store: store, draft: privacyDraft)
            case .data: DataSettingsPage(store: store, actions: actions, cleanup: cleanup)
            case .advanced: AdvancedSettingsPage(store: store, actions: actions, output: advancedOutput)
            case .about: AboutSettingsPage(store: store)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
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
    @AppStorage("notch.hoverEnabled") private var hoverEnabled = true
    @AppStorage(NotchModel.sendOnReleaseKey) private var sendOnRelease = false
    var onHoverEnabledChange: ((Bool) -> Void)?

    var body: some View {
        Form {
            if let loginItems {
                Section("Startup") { LoginItemRow(loginItems: loginItems) }
            }
            if let updates {
                Section("Updates") { UpdatesSection(updates: updates) }
            }
            Section("Notch") {
                // The keys come from the app guide (NotchCore AppGuide), the list FERB's self-description also reads.
                LabeledContent("Open / close FERB", value: AppGuide.keys("Open / close FERB"))
                LabeledContent("Dictation", value: AppGuide.keys("Dictation"))
                Toggle("Send dictation when I let go of the keys", isOn: $sendOnRelease)
                Text("A dictated message is answered aloud as well as in the chat.")
                    .font(.caption).foregroundStyle(.secondary)
                LabeledContent("Submit / new line", value: AppGuide.keys("Submit / new line"))
                Toggle("Open when I hover over FERB", isOn: $hoverEnabled)
                    .onChange(of: hoverEnabled) { _, enabled in onHoverEnabledChange?(enabled) }
                Text("You can change this any time here, or ask FERB to turn hover opening on or off.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Section("FERB's pointer") {
                Toggle("Show FERB's pointer when it acts on screen", isOn: $cursorEnabled)
                    .onChange(of: cursorEnabled) { _, on in cursor.enabled = on }
            }
            Section("Sounds") {
                Toggle("Play feedback sounds", isOn: $feedback.enabled)
                Toggle("Magnetic docking sound", isOn: $feedback.magneticEnabled).disabled(!feedback.enabled)
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

    func uninstall(with actions: FERBAppActions, purge: Bool = false) {
        actions.beforeUninstall?()
        run(Self.uninstallArguments(purge: purge), with: actions)
    }

    /// `ferb uninstall`, or with everything deleted after the owner's one Yes here (`--purge --yes`).
    static func uninstallArguments(purge: Bool) -> [String] { purge ? ["uninstall", "--purge", "--yes"] : ["uninstall"] }
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
/// service answers, and signing FERB's own Codex or Claude out. This page never shows or sends a key (rule 14).
struct AISettingsPage: View {
    @ObservedObject var store: FERBWindowStore
    let actions: FERBAppActions
    @ObservedObject var output: FerbOutput

    init(store: FERBWindowStore, actions: FERBAppActions, output: FerbOutput? = nil) {
        self.store = store
        self.actions = actions
        self.output = output ?? FerbOutput()
    }

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
                    Button("Sign out of Claude") {          // FERB's own Claude sign-in only (`ferb setup --claude-logout`)
                        output.run(["setup", "--claude-logout"], with: actions) { Task { await setup.refreshAI() } }
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
            Form {
                BrowsersStepView(model: setup)
                Section("FERB's own account") { IdentityStepView(model: setup, showsHeader: false) }
            }
                .formStyle(.grouped)
                .task { await setup.refreshBrowsers(); await setup.refreshIdentity() }
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
@MainActor
final class PrivacySettingsDraft: ObservableObject {
    @Published var newApp = ""
    @Published var newSite = ""
}

struct PrivacySettingsPage: View {
    @ObservedObject var store: FERBWindowStore
    @ObservedObject var draft: PrivacySettingsDraft

    init(store: FERBWindowStore, draft: PrivacySettingsDraft? = nil) {
        self.store = store
        self.draft = draft ?? PrivacySettingsDraft()
    }

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
                                 newItem: $draft.newApp) { list in Task { await store.set("privacy.exclude_apps", .strings(list)) } }
            }
            Section("Sites FERB ignores") {
                StringListEditor(items: store.settings.strings("privacy.exclude_domains"),
                                 placeholder: "Site, e.g. bank.example.com",
                                 newItem: $draft.newSite) { list in Task { await store.set("privacy.exclude_domains", .strings(list)) } }
            }
            Section("How long FERB keeps records") {
                DaysStepper(title: "Activity trail", key: "sense.trail_retention_days", store: store)
                DaysStepper(title: "Task traces", key: "retention.trace_days", store: store)
                DaysStepper(title: "Jev decisions", key: "retention.decision_days", store: store)
                DaysStepper(title: "Screenshots and evidence", key: "retention.evidence_days", store: store)
                DaysStepper(title: "Recovery copies", key: "retention.recovery_days", store: store)
                DaysStepper(title: "Resource log", key: "retention.resource_days", store: store)
                DaysStepper(title: "Operational records", key: "retention.operational_days", store: store)
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
    @ObservedObject var output: FerbOutput
    @State private var confirmUninstall = false
    @State private var confirmPurge = false

    private var logs: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".ferb/logs", isDirectory: true)
    }

    init(store: FERBWindowStore, actions: FERBAppActions, output: FerbOutput? = nil) {
        self.store = store
        self.actions = actions
        self.output = output ?? FerbOutput()
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
                    Button("Uninstall FERB and delete all its data…", role: .destructive) { confirmPurge = true }
                        .disabled(output.running)
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
        // Permanent (decision 8): asked once here; the command then runs without another question.
        .alert("Uninstall FERB and delete everything?", isPresented: $confirmPurge) {
            Button("Delete Everything", role: .destructive) { output.uninstall(with: actions, purge: true) }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This can't be undone. FERB stops and deletes ~/.ferb (its memory, history, settings and FERB's own " +
                 "browser data) and every FERB item in your Keychain: API keys, the secrets and cards you saved, and " +
                 "FERB's sign-ins. Then quit FERB and drag it to the Trash.")
        }
    }
}


// MARK: - Step 12c screens (functional only; the owner polishes them)

/// Settings → About (decision 6): FERB's self-description from the daemon (`about.get`), built from code and config, and
/// this app's own how-tos and shortcuts. Read-only: every setting is changed on its own page.
struct AboutSettingsPage: View {
    @ObservedObject var store: FERBWindowStore

    var body: some View {
        Form {
            if let about = store.about {
                Section("FERB") {
                    LabeledContent("Version", value: about.version)
                    ForEach(about.models) { LabeledContent("Model for the \($0.name)", value: $0.value) }
                    ForEach(about.account) { line in
                        LabeledContent(line.name, value: line.value)
                        if !line.note.isEmpty { Text(line.note).font(.caption).foregroundStyle(.secondary) }
                    }
                }
                Section("What FERB keeps about you, where, and for how long") {
                    ForEach(about.records) { line in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(line.name.prefix(1).uppercased() + line.name.dropFirst())
                            Text("\(line.value) · \(line.note)").font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
                Section("Permissions") { ForEach(about.permissions) { LabeledContent($0.name, value: $0.value) } }
                Section("Your settings") {
                    ForEach(about.settings) { line in
                        VStack(alignment: .leading, spacing: 2) {
                            LabeledContent(line.name, value: line.value)
                            Text(line.note).font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
                Section("Health") {
                    ForEach(about.health) { line in
                        LabeledContent(line.name, value: line.value)
                        if !line.note.isEmpty { Text(line.note).font(.caption).foregroundStyle(.secondary) }
                    }
                }
                Section("Cost") {
                    ForEach(about.cost) { LabeledContent($0.name, value: $0.value) }
                    if let basis = about.cost.first?.note { Text(basis).font(.caption).foregroundStyle(.secondary) }
                }
            } else {
                ProgressView()
            }
            Section("Shortcuts") { ForEach(AppGuide.shortcuts, id: \.action) { LabeledContent($0.action, value: $0.keys) } }
            Section("How to") { ForEach(AppGuide.howtos, id: \.title) { LabeledContent($0.title, value: $0.steps) } }
        }
        .formStyle(.grouped)
        .task { await store.loadAbout() }
    }
}

/// Settings → Data (decision 8, storage decision 4): put back what FERB changed or trashed, delete a saved secret or
/// card, and the one-time clean-up of data FERB no longer uses.
struct DataSettingsPage: View {
    @ObservedObject var store: FERBWindowStore
    let actions: FERBAppActions
    @ObservedObject var cleanup: CleanupRunner
    @State private var deleting: SavedSecretRow?

    var body: some View {
        Form {
            Section("Restore") {
                if let line = store.restoreLine { Text(line).font(.caption).foregroundStyle(.secondary) }
                if store.restoreRows.isEmpty { Text("Nothing to put back.").foregroundStyle(.secondary) }
                ForEach(store.restoreRows) { row in
                    HStack {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(row.original).textSelection(.enabled)
                            Text([row.what, row.at?.formatted(date: .abbreviated, time: .shortened) ?? ""]
                                .filter { !$0.isEmpty }.joined(separator: " · "))
                                .font(.caption).foregroundStyle(.secondary)
                            if !row.restorable {
                                Text("Open it from macOS Recovery or Time Machine (\(row.savedTo)).")
                                    .font(.caption).foregroundStyle(.secondary)
                            }
                        }
                        Spacer()
                        if row.restorable { Button("Restore") { Task { await store.restore(row) } } }
                    }
                }
            }
            Section("Saved secrets and cards") {
                Text("Names only: FERB never shows a saved value.").font(.caption).foregroundStyle(.secondary)
                if store.savedSecrets.isEmpty { Text("Nothing saved.").foregroundStyle(.secondary) }
                ForEach(store.savedSecrets) { row in
                    HStack {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(row.name)
                            Text("\(row.kindLabel) · \(row.place)" + (row.unusable ? " · can't be used any more" : ""))
                                .font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Button("Delete…", role: .destructive) { deleting = row }
                    }
                }
            }
            Section("Clean up") { CleanupSection(store: store, actions: actions, cleanup: cleanup) }
        }
        .formStyle(.grouped)
        .task {
            await store.loadRestore()
            await store.loadSecrets()
            await store.checkHealth()
        }
        // Permanent: asked once here.
        .alert("Delete this saved secret?", isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }),
               presenting: deleting) { row in
            Button("Delete", role: .destructive) { Task { await store.deleteSecret(row); deleting = nil } }
            Button("Cancel", role: .cancel) { deleting = nil }
        } message: { row in
            Text("“\(row.name)” for \(row.place) is deleted from your Keychain. This can't be undone; FERB asks you again " +
                 "the next time it needs it.")
        }
    }
}

/// The one-time clean-up's screen (storage decision 4): `ferb setup --cleanup` lists what it removes; OK runs it with
/// the digest of exactly that list (`--ok`), the daemon stopped meanwhile; then the result.
@MainActor
final class CleanupRunner: ObservableObject {
    enum Phase: Equatable { case idle, reading, plan(CleanupPlan), running, done(CleanupResult), failed(String) }
    @Published private(set) var phase: Phase = .idle
    @Published private(set) var lines: [String] = []

    func preview(with actions: FERBAppActions) {
        guard let runFerb = actions.runFerb else { return }
        phase = .reading
        lines = []
        let started = runFerb(["setup", "--cleanup"], { [weak self] line in self?.lines.append(line) }) { [weak self] status in
            guard let self else { return }
            if let plan = CleanupPlan(FerbJSON.last(self.lines)), status == 0 { self.phase = .plan(plan) }
            else { self.phase = .failed(self.lines.last ?? "FERB couldn't read what the clean-up would remove.") }
        }
        if !started { phase = .failed("FERB is already running a setup step; try again when it finishes.") }
    }

    func apply(_ plan: CleanupPlan, with actions: FERBAppActions) {
        guard let runFerb = actions.runFerb, let arguments = plan.applyArguments else { return }
        phase = .running
        lines = []
        let started = runFerb(arguments, { [weak self] line in self?.lines.append(line) }) { [weak self] status in
            guard let self else { return }
            self.phase = .done(CleanupResult(FerbJSON.last(self.lines), status: status))
        }
        if !started { phase = .failed("FERB is already running a setup step; try again when it finishes.") }
    }
}

struct CleanupSection: View {
    @ObservedObject var store: FERBWindowStore
    let actions: FERBAppActions
    @ObservedObject var cleanup: CleanupRunner

    private var waiting: Bool { store.health.contains { $0.name == "Database" && !$0.ok } }

    var body: some View {
        if waiting { Text("A one-time clean-up of data FERB no longer uses waits for your OK.").bold() }
        if actions.runFerb == nil {
            Text("This is run from the installed FERB.app.").foregroundStyle(.secondary)
        } else {
            switch cleanup.phase {
            case .idle:
                Button("Show what the clean-up removes") { cleanup.preview(with: actions) }
            case .reading:
                ProgressView("Reading FERB's database…")
            case .plan(let plan):
                if plan.nothingToDo {
                    Text("Nothing to clean up.").foregroundStyle(.secondary)
                } else {
                    ForEach(plan.steps) { step in
                        LabeledContent(step.what, value: "\(step.rows) rows")
                    }
                    Text("\(plan.rows) rows in all. Your memory, tasks, chats, schedules and recent traces are never touched.")
                        .font(.caption).foregroundStyle(.secondary)
                    Text("FERB's database: \(shownBytes(plan.bytes)); " + (plan.compact
                        ? "then compacted once (needs \(shownBytes(plan.bytes)) free; \(shownBytes(plan.free)) free now)."
                        : "already compacted."))
                        .font(.caption).foregroundStyle(.secondary)
                    Text("FERB stops while it runs (a few minutes) and starts again by itself.")
                        .font(.caption).foregroundStyle(.secondary)
                    Button("OK, clean up") { cleanup.apply(plan, with: actions) }
                }
            case .running:
                ProgressView(cleanup.lines.last ?? "Cleaning up…")
            case .done(let result):
                Text(result.line).textSelection(.enabled)
                Button("Check again") { cleanup.preview(with: actions) }
            case .failed(let why):
                Text(why).foregroundStyle(.secondary).textSelection(.enabled)
                Button("Try again") { cleanup.preview(with: actions) }
            }
        }
    }
}
