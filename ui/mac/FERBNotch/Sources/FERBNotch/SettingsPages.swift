import AppKit
import SwiftUI
import NotchCore

enum SettingsTab: Hashable, CaseIterable, Identifiable {
    case general, notch, notifications, ai, browsers, permissions, privacy, data, advanced, about

    var id: SettingsTab { self }

    var title: String {
        switch self {
        case .general: return "General"
        case .notch: return "Notch & Shortcuts"
        case .ai: return "AI"
        case .permissions: return "Permissions"
        case .browsers: return "Browsers"
        case .notifications: return "Sounds & Notifications"
        case .privacy: return "Privacy"
        case .data: return "Data"
        case .advanced: return "Advanced"
        case .about: return "About"
        }
    }

    var subtitle: String {
        switch self {
        case .general: return "Launch, updates and a fresh introduction."
        case .notch: return "How FERB opens, listens and stays out of your way."
        case .notifications: return "Choose what gets your attention and how it sounds."
        case .ai: return "Choose the AI service FERB thinks with."
        case .browsers: return "Connect the browsers and accounts FERB can use."
        case .permissions: return "The access FERB needs to work on your Mac."
        case .privacy: return "What FERB can notice, and what stays private."
        case .data: return "Your saved information and recovery options."
        case .advanced: return "Diagnostics and maintenance when you need them."
        case .about: return "Your FERB version and the services behind it."
        }
    }

    var symbol: String {
        switch self {
        case .general: return "gearshape"
        case .notch: return "macwindow"
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
                GeneralSettingsPage(loginItems: actions.loginItems, updates: actions.updates, onReplayIntro: actions.replayIntro)
            case .notch: NotchSettingsPage(cursor: actions.cursor, onHoverEnabledChange: actions.setHoverEnabled)
            case .ai: AISettingsPage(store: store, actions: actions, output: aiOutput)
            case .permissions: PermissionsSettingsPage(actions: actions)
            case .browsers: BrowsersSettingsPage(actions: actions)
            case .notifications: NotificationsSettingsPage(store: store, feedback: actions.feedback, openSettings: actions.openNotificationSettings)
            case .privacy: PrivacySettingsPage(store: store, draft: privacyDraft)
            case .data: DataSettingsPage(store: store, actions: actions, cleanup: cleanup)
            case .advanced: AdvancedSettingsPage(store: store, actions: actions, output: advancedOutput)
            case .about: AboutSettingsPage(store: store)
            }
        }
        .scrollContentBackground(.hidden)
        .toggleStyle(.switch)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

/// General holds app lifecycle choices; daily interaction has its own visible destination.
struct GeneralSettingsPage: View {
    var loginItems: LoginItemController? = nil
    var updates: UpdateCoordinator? = nil
    var onReplayIntro: (() -> Void)? = nil

    var body: some View {
        WindowPage {
            if let loginItems {
                WindowSettingsSection(title: "Startup") { LoginItemRow(loginItems: loginItems) }
            }
            if let updates {
                WindowSettingsSection(title: "Updates") { UpdatesSection(updates: updates) }
            }
            if let onReplayIntro {
                WindowSettingsSection(title: "Meet FERB again") {
                    HStack(spacing: 24) {
                        Text("Replay the introduction whenever you like. Your setup stays saved.")
                            .font(.system(size: 12)).foregroundStyle(.secondary)
                        Spacer(minLength: 0)
                        Button("Replay intro", action: onReplayIntro)
                    }
                }
            }
        }
    }
}

struct NotchSettingsPage: View {
    let cursor: CursorController
    var onHoverEnabledChange: ((Bool) -> Void)?
    // Each control's key and name come from its one declaration (AppSettings), which FERB's guide reads too
    @AppStorage(AppSettings.pointer.key) private var cursorEnabled = true
    @AppStorage(AppSettings.hover.key) private var hoverEnabled = true
    @AppStorage(AppSettings.sendOnRelease.key) private var sendOnRelease = false

    var body: some View {
        WindowPage {
            WindowSettingsSection(title: "Notch & pill") {
                Toggle(AppSettings.hover.name, isOn: $hoverEnabled)
                    .onChange(of: hoverEnabled) { _, enabled in onHoverEnabledChange?(enabled) }
                Text("Drag FERB away from the notch to leave its pill on any screen edge.")
                    .font(.system(size: 12)).foregroundStyle(.secondary)
            }
            WindowSettingsSection(title: "Keyboard & voice") {
                ForEach(AppGuide.shortcuts.filter(\.inNotchSettings), id: \.action) { shortcut in
                    LabeledContent(shortcut.action, value: shortcut.keys())
                }
                Toggle(AppSettings.sendOnRelease.name, isOn: $sendOnRelease)
                Text("FERB replies in the chat, in text. It never speaks.")
                    .font(.system(size: 12)).foregroundStyle(.secondary)
            }
            WindowSettingsSection(title: "FERB’s pointer") {
                Toggle(AppSettings.pointer.name, isOn: $cursorEnabled)
                    .onChange(of: cursorEnabled) { _, on in cursor.enabled = on }
            }
        }
    }
}

struct SoundSettingsControls: View {
    @ObservedObject var feedback: SoundFeedback
    var body: some View {
        Toggle(AppSettings.soundsOn.name, isOn: $feedback.enabled)
        Slider(value: $feedback.volume, in: 0...0.5) { Text(AppSettings.volume.name) }.disabled(!feedback.enabled)
        Picker(AppSettings.intensity.name, selection: $feedback.subtle) {
            Text("Standard").tag(false)
            Text("Subtle").tag(true)
        }.pickerStyle(.segmented).disabled(!feedback.enabled)
        Text("During a call FERB plays only the “needs you” sound, softly; the notch still shows everything.")
            .font(.system(size: 12)).foregroundStyle(.secondary)
        DisclosureGroup("Individual sounds") {
            VStack(alignment: .leading, spacing: 18) {
                ForEach(SoundSwitch.allCases, id: \.self) { sound in
                    HStack(spacing: 14) {
                        VStack(alignment: .leading, spacing: 3) {
                            Text(sound.title)
                            Text(sound.meaning).font(.system(size: 11)).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Button("Play") { feedback.preview(sound) }.disabled(!feedback.isOn(sound))
                        Toggle(sound.title, isOn: feedback.binding(sound)).labelsHidden()
                    }.disabled(!feedback.enabled)
                }
            }.padding(.top, 16)
        }
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
        guard let before = actions.beforeUninstall else {
            run(Self.uninstallArguments(purge: purge), with: actions)
            return
        }
        guard !running else { return }
        lines = []
        status = nil
        running = true
        Task { @MainActor in
            let refused = await before()                  // the background service off first, waited for
            running = false
            if let refused { lines = [refused]; return }
            run(Self.uninstallArguments(purge: purge), with: actions)
        }
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
            WindowPage {
                WindowSettingsSection { AIStepView(model: setup) }
                WindowSettingsSection(title: "Check") {
                    Button(store.checkingRuntime ? "Checking…" : "Check connection") {
                        Task { await store.checkRuntime(setup.ai?.current ?? "codex") }
                    }.disabled(store.checkingRuntime || output.running)
                    if let result = store.runtimeResult {
                        Text(result).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                    }
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
            .task { await setup.refreshAI() }
        } else {
            WindowPage { WindowSettingsSection { OnlyInTheApp() } }
        }
    }
}

/// Settings → Permissions: plan C's rows, each as FERB's background service (or FERB.app) itself sees it, with Grant
/// and the System Settings pane; it re-reads while shown.
struct PermissionsSettingsPage: View {
    let actions: FERBAppActions

    var body: some View {
        if let setup = actions.setup {
            WindowPage { WindowSettingsSection { PermissionsStepView(model: setup) } }
                .onAppear { setup.startPolling(.permissions) }
                .onDisappear { setup.stopPolling() }
        } else {
            WindowPage { WindowSettingsSection { OnlyInTheApp() } }
        }
    }
}

/// Settings → Browsers: plan C's browsers and profiles (not installed / installed / on / paired) and Add to….
struct BrowsersSettingsPage: View {
    let actions: FERBAppActions

    var body: some View {
        if let setup = actions.setup {
            WindowPage {
                WindowSettingsSection {
                    BrowsersStepView(model: setup)
                    // Pair all's result and Add to's outcome (HAND-411, HAND-413); the setup window shows its own
                    if let message = setup.message { Text(message).font(.callout).foregroundStyle(.secondary) }
                }
                WindowSettingsSection(title: "FERB's own account") { IdentityStepView(model: setup, showsHeader: false) }
            }
            .task { await setup.refreshBrowsers(); await setup.refreshIdentity() }
            .onDisappear { setup.stopWatchingForExtension() }
        } else {
            WindowPage { WindowSettingsSection { OnlyInTheApp() } }
        }
    }
}

/// Settings → Sounds & Notifications: which kinds FERB posts while the notch is closed (plan D's categories).
struct NotificationsSettingsPage: View {
    @ObservedObject var store: FERBWindowStore
    @ObservedObject var feedback: SoundFeedback
    var openSettings: () -> Void

    var body: some View {
        WindowPage {
            WindowSettingsSection(title: "Notify me about") {
                ForEach(BannerCategory.allCases, id: \.rawValue) { category in
                    Toggle(category.title, isOn: store.banner(category))
                }
                Button("Open macOS notification settings", action: openSettings)
            }
            WindowSettingsSection(title: "Sounds") { SoundSettingsControls(feedback: feedback) }
        }
    }
}

/// Settings → Privacy: what FERB records, the apps and sites it ignores, how long it keeps records, Pause recording.
@MainActor
final class PrivacySettingsDraft: ObservableObject {
    let apps = SettingsListDraft()
    let sites = SettingsListDraft()
}

struct PrivacySettingsPage: View {
    @ObservedObject var store: FERBWindowStore
    @ObservedObject var draft: PrivacySettingsDraft

    init(store: FERBWindowStore, draft: PrivacySettingsDraft? = nil) {
        self.store = store
        self.draft = draft ?? PrivacySettingsDraft()
    }

    var body: some View {
        WindowPage {
            WindowSettingsSection(title: "Recording") {
                Toggle("Pause recording what happens on this Mac",
                       isOn: Binding(get: { store.recordingPaused }, set: { on in Task { await store.pauseRecording(on) } }))
                Toggle("Record window titles", isOn: store.toggle("sense.window_titles"))
                LabeledContent("Recording now", value: store.watchers.isEmpty ? "nothing" : store.watchers.joined(separator: ", "))
            }
            WindowSettingsSection(title: "Apps FERB ignores") {
                StringListEditor(key: "privacy.exclude_apps", placeholder: "Bundle id or app name, e.g. com.1password.1password",
                                 store: store, draft: draft.apps)
            }
            WindowSettingsSection(title: "Sites FERB ignores") {
                StringListEditor(key: "privacy.exclude_domains", placeholder: "Site, e.g. bank.example.com",
                                 store: store, draft: draft.sites)
            }
            WindowSettingsSection(title: "How long FERB keeps records") {
                DaysField(title: "Activity trail", key: "sense.trail_retention_days", store: store)
                DaysField(title: "Task traces", key: "retention.trace_days", store: store)
                DaysField(title: "Jev decisions", key: "retention.decision_days", store: store)
                DaysField(title: "Screenshots and evidence", key: "retention.evidence_days", store: store)
                DaysField(title: "Recovery copies", key: "retention.recovery_days", store: store)
                DaysField(title: "Resource log", key: "retention.resource_days", store: store)
                DaysField(title: "Operational records", key: "retention.operational_days", store: store)
            }
        }
    }
}

/// Settings → Advanced: logs, a health check, the diagnostic file, the command-line tool, uninstall.
struct AdvancedSettingsPage: View {
    @ObservedObject var store: FERBWindowStore
    let actions: FERBAppActions
    @ObservedObject var output: FerbOutput
    @State private var confirmUninstall = false
    @State private var confirmPurge = false

    private var logs: URL { actions.logsDirectory }

    init(store: FERBWindowStore, actions: FERBAppActions, output: FerbOutput? = nil) {
        self.store = store
        self.actions = actions
        self.output = output ?? FerbOutput()
    }

    var body: some View {
        WindowPage {
            WindowSettingsSection(title: "Logs") {
                Button("Open FERB's logs folder") { NSWorkspace.shared.open(logs) }
            }
            WindowSettingsSection(title: "Health") {
                Button(store.checkingHealth ? "Checking…" : "Check FERB") { Task { await store.checkHealth() } }
                    .disabled(store.checkingHealth)
                if let problem = store.healthProblem { Text(problem).font(.caption).foregroundStyle(.red).textSelection(.enabled) }
                ForEach(store.health) { line in LabeledContent(line.name, value: line.ok ? "OK" : line.detail) }
            }
            WindowSettingsSection(title: "Diagnostic file") {
                Button("Export a diagnostic file…") {
                    Task {
                        if let url = await store.exportDiagnostic(appVersion: actions.appVersion) {
                            NSWorkspace.shared.activateFileViewerSelecting([url])
                        }
                    }
                }
                if let report = store.diagnostic?.report {
                    HStack {
                        Button("View diagnostic file") { store.open(report.file) }
                        Button(store.diagnostic?.isUnsent == true ? "Send again" : "Send") {
                            Task { await store.sendDiagnostic(appVersion: actions.appVersion) }
                        }
                        .disabled(store.diagnostic == .sending(report))
                    }
                } else {
                    Button(store.diagnostic == .preparing ? "Preparing…" : "Send diagnostics to developer…") {
                        Task { await store.prepareDiagnostic(appVersion: actions.appVersion) }
                    }
                    .disabled(store.diagnostic == .preparing)
                }
                if let line = store.diagnostic?.line { Text(line).textSelection(.enabled) }
                Text("Written on this Mac with secrets removed. Nothing is sent until you view it and press Send.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            if actions.runFerb != nil {
                WindowSettingsSection(title: "Command-line tool and uninstall") {
                    Button("Install command-line tool") { output.run(["setup", "--cli"], with: actions) }
                        .disabled(output.running)
                    Button("Uninstall FERB…") { confirmUninstall = true }.disabled(output.running)
                    Button("Uninstall FERB and delete all its data…", role: .destructive) { confirmPurge = true }
                        .disabled(output.running)
                    OutputLines(output: output)
                }
            }
        }
        .alert("Uninstall FERB?", isPresented: $confirmUninstall) {
            Button("Uninstall", role: .destructive) { output.uninstall(with: actions) }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("FERB stops its background service and removes its browser connection, its login item and the `ferb` " +
                 "link. Your data in ~/.ferb stays. Then quit FERB and drag it to the Trash.")
        }
        // Permanent (APP-2): asked once here; the command then runs without another question.
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

/// Settings → About (DMN-6): FERB's self-description from the daemon (`about.get`), built from code and config, and
/// this app's own how-tos and shortcuts. Read-only: every setting is changed on its own page.
struct AboutSettingsPage: View {
    @ObservedObject var store: FERBWindowStore

    var body: some View {
        WindowPage {
            if let about = store.about {
                WindowSettingsSection(title: "FERB") {
                    LabeledContent("Version", value: about.version)
                    ForEach(about.models) { LabeledContent("Model for the \($0.name)", value: $0.value) }
                    ForEach(about.account) { line in
                        LabeledContent(line.name, value: line.value)
                        if !line.note.isEmpty { Text(line.note).font(.caption).foregroundStyle(.secondary) }
                    }
                }
                WindowSettingsSection(title: "What FERB keeps about you, where, and for how long") {
                    ForEach(about.records) { line in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(line.name.prefix(1).uppercased() + line.name.dropFirst())
                            Text("\(line.value) · \(line.note)").font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
                WindowSettingsSection(title: "Permissions") { ForEach(about.permissions) { LabeledContent($0.name, value: $0.value) } }
                WindowSettingsSection(title: "Your settings") {
                    // FERB's settings, then FERB.app's own as FERB knows them now (APP-728)
                    ForEach(about.settings + about.appSettings) { line in
                        VStack(alignment: .leading, spacing: 2) {
                            LabeledContent(line.name, value: line.value)
                            Text(line.note).font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
                WindowSettingsSection(title: "Health") {
                    ForEach(about.health) { line in
                        LabeledContent(line.name, value: line.value)
                        if !line.note.isEmpty { Text(line.note).font(.caption).foregroundStyle(.secondary) }
                    }
                }
                WindowSettingsSection(title: "Cost") {
                    ForEach(about.cost) { LabeledContent($0.name, value: $0.value) }
                    if let basis = about.cost.first?.note { Text(basis).font(.caption).foregroundStyle(.secondary) }
                }
            } else {
                ProgressView()
            }
            WindowSettingsSection(title: "Shortcuts") { ForEach(AppGuide.shortcuts, id: \.action) { LabeledContent($0.action, value: $0.keys()) } }
            WindowSettingsSection(title: "How to") { ForEach(AppGuide.howtos, id: \.title) { LabeledContent($0.title, value: $0.steps) } }
            WindowSettingsSection(title: "Sounds") { SoundCredits() }
        }
        .task { await store.loadAbout() }
    }
}

/// The credit Creative Commons Attribution asks for (demo/sound-family/CREDITS.md): two of FERB's sounds are Google's.
private struct SoundCredits: View {
    var body: some View {
        Text("FERB's sounds are made for FERB from code, except two from Google's Material Design sound resources, by "
             + "Google, licensed under Creative Commons Attribution 4.0 International (CC BY 4.0).")
        Text("First-launch sound: Material's \"hero\" sound. Changes: leading silence trimmed, leveled to −24 LUFS.")
            .font(.caption).foregroundStyle(.secondary)
        Text("Notification sound: Material's \"notification, high intensity\". Changes: trimmed, leveled, a small "
             + "room added; the Subtle version is also low-passed at 3 kHz.")
            .font(.caption).foregroundStyle(.secondary)
        Link("Licence: creativecommons.org/licenses/by/4.0", destination: URL(string: "https://creativecommons.org/licenses/by/4.0/")!)
        Link("Source: github.com/nana-4/materia-sound-theme", destination: URL(string: "https://github.com/nana-4/materia-sound-theme")!)
    }
}

/// Settings → Data (APP-2, STORE-402): put back what FERB changed or trashed, delete a saved secret or
/// card and the one-time clean-up of data FERB no longer uses.
struct DataSettingsPage: View {
    @ObservedObject var store: FERBWindowStore
    let actions: FERBAppActions
    @ObservedObject var cleanup: CleanupRunner
    @State private var deleting: SavedSecretRow?

    var body: some View {
        WindowPage {
            WindowSettingsSection(title: "Restore") {
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
            WindowSettingsSection(title: "Saved secrets and cards") {
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
            WindowSettingsSection(title: "Clean up") { CleanupSection(store: store, actions: actions, cleanup: cleanup) }
        }
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

/// The one-time clean-up's screen (STORE-402): `ferb setup --cleanup` lists what it removes; OK runs it with
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
                        ? "then compacted once (needs up to \(shownBytes(plan.needed)) free; \(shownBytes(plan.free)) free now)."
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
