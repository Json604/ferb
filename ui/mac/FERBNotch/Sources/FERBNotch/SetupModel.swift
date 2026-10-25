import Foundation
import NotchCore

/// The setup window's state and actions (L34 C). Every fact comes from the daemon's setup RPCs or FERB.app's own
/// permission APIs; the window decides no grant and no choice itself.
@MainActor
final class SetupModel: ObservableObject {
    @Published var step: SetupStep = .welcome
    @Published var serviceAction: AppSetupAction = .none
    @Published private(set) var serviceRunning = false
    @Published private(set) var serviceLine = ""
    @Published private(set) var serviceError: String?
    @Published private(set) var ai: AISnapshot?
    @Published private(set) var aiMessage: String?
    @Published private(set) var signingIn = false
    @Published private(set) var claudeSigningIn = false
    @Published private(set) var permissions: PermissionSnapshot?
    @Published private(set) var hearingRow: PermissionRow?
    @Published private(set) var appRows: [PermissionRow] = []
    @Published private(set) var askedRestart: Set<String> = []
    @Published private(set) var restarting = false
    @Published private(set) var browsers: BrowserSnapshot?
    @Published private(set) var persona: PersonaState = .loading
    @Published private(set) var personaMessage: String?
    @Published var message: String?

    let rpc: DaemonRPCTransport
    let runner: SetupCommandRunning
    let app: AppPermissionsProviding
    let system: SystemOpening
    let keys: APIKeyStoring
    let service: DaemonServicing
    var onFinish: (() -> Void)?
    private var poll: Task<Void, Never>?

    static let notAnswering = "FERB isn't answering right now; this step updates when it's back."

    init(rpc: DaemonRPCTransport, runner: SetupCommandRunning, app: AppPermissionsProviding, system: SystemOpening,
         keys: APIKeyStoring, service: DaemonServicing) {
        self.rpc = rpc
        self.runner = runner
        self.app = app
        self.system = system
        self.keys = keys
        self.service = service
    }

    /// What the setup window asks for: what every task needs. The rest are asked when first needed (owner 2026-10-26:
    /// "ask when first needed, like Claude and ChatGPT"); Settings → Permissions lists them all.
    static let askedAtSetup: Set<String> = ["accessibility", "screen_recording"]
    var setupRows: [PermissionRow] { allRows.filter { Self.askedAtSetup.contains($0.id) } }

    var allRows: [PermissionRow] { (permissions?.rows ?? []) + appRows + (hearingRow.map { [$0] } ?? []) }

    var offersRestart: Bool { allRows.contains { askedRestart.contains($0.id) && $0.status != .granted } }

    var showsFullDiskHint: Bool {
        askedRestart.contains("full_disk_access")
            && allRows.contains { $0.id == "full_disk_access" && $0.status != .granted }
    }

    var primaryTitle: String {
        switch step {
        case .welcome:
            if serviceError != nil { return "Try Again" }
            return AppSetupPlan.serviceArguments(for: serviceAction) == nil ? "Continue" : "Set Up FERB"
        case .done: return "Open FERB"
        default: return "Continue"
        }
    }

    var primaryDisabled: Bool { serviceRunning }

    // MARK: Steps

    /// The step a Grant left when macOS may restart FERB (Screen Recording's "Quit & Reopen"): reopened at the next launch,
    /// cleared on leaving that step (owner 2026-10-26: setup didn't come back after the restart).
    static let resumeKey = "FERBSetupResumeStep"
    static func pendingResume(_ defaults: UserDefaults) -> SetupStep? {
        (defaults.object(forKey: resumeKey) as? Int).flatMap(SetupStep.init(rawValue:))
    }

    func go(_ next: SetupStep) {
        if next != .permissions { UserDefaults.standard.removeObject(forKey: Self.resumeKey) }
        step = next
        message = nil
        Task { await self.enter(next) }
    }

    /// What a step reads on entering it; polling keeps it fresh afterwards.
    func enter(_ step: SetupStep) async {
        switch step {
        case .permissions: await refreshPermissions(probeHearing: true)
        case .persona: await refreshPersona()
        default: await refreshCurrentStep()
        }
    }

    func primary() async {
        switch step {
        case .welcome: continueFromWelcome()
        case .done: await finish()
        default: if let next = step.next { go(next) }
        }
    }

    func continueFromWelcome() {
        guard AppSetupPlan.serviceArguments(for: serviceAction) != nil else { go(.ai); return }
        runService { [weak self] ok in if ok { self?.go(.ai) } }
    }

    /// `ferb setup --service [--update]`: the background service from this app, no questions. On failure its last lines
    /// (the reason and its fix) stay on Welcome.
    func runService(completion: @escaping (Bool) -> Void) {
        guard let arguments = AppSetupPlan.serviceArguments(for: serviceAction) else { completion(true); return }
        serviceError = nil
        serviceLine = "Starting…"
        serviceRunning = true
        var tail: [String] = []
        let started = runner.run(arguments, line: { [weak self] text in
            self?.serviceLine = text
            tail = Array((tail + [text]).suffix(6))
        }, done: { [weak self] status in
            guard let self else { return }
            self.serviceRunning = false
            if status == 0 {
                self.serviceAction = .none
                completion(true)
            } else {
                self.serviceError = tail.isEmpty ? "FERB's setup command stopped (exit status \(status))." :
                    tail.joined(separator: "\n")
                completion(false)
            }
        })
        if !started {
            serviceRunning = false
            serviceError = "FERB couldn't start its setup command."
            completion(false)
        }
    }

    // MARK: Polling

    func startPolling() {
        poll?.cancel()
        poll = Task { [weak self] in
            while !Task.isCancelled {
                await self?.refreshCurrentStep()
                do { try await Task.sleep(for: .seconds(2)) } catch { return }
            }
        }
    }

    func stopPolling() {
        poll?.cancel()
        poll = nil
    }

    func refreshCurrentStep() async {
        switch step {
        case .ai: await refreshAI()
        case .permissions: await refreshPermissions()
        case .browsers: await refreshBrowsers()
        case .persona: if persona == .loading || persona == .scanning { await refreshPersona() }
        case .welcome, .done: break
        }
    }

    private func clearNotAnswering() { if message == Self.notAnswering { message = nil } }

    // MARK: Choose your AI

    func refreshAI() async {
        do {
            ai = AISnapshot(reply: try await rpc.call("runtimes.status", params: [:]))
            clearNotAnswering()
        } catch {
            message = Self.notAnswering
            return
        }
        guard claudeSigningIn, let status = claudeSignIn, !status.waiting else { return }
        claudeSigningIn = false                        // the daemon's Claude sign-in ended
        if status.state == "signed_in" { await use("claude", check: true) }
        else { aiMessage = status.why.isEmpty ? "Not signed in to Claude." : status.why }
    }

    func signInWithChatGPT() {
        guard !runner.isRunning else { aiMessage = "Wait for the current step to finish."; return }
        signingIn = true
        aiMessage = "Finish signing in in the browser window that opened."
        let started = runner.run(AppSetupPlan.codexLoginArguments, line: { _ in }, done: { [weak self] status in
            guard let self else { return }
            self.signingIn = false
            if status == 0 { Task { await self.use("codex", check: false) } }
            else { self.aiMessage = "Not signed in. Try again when you're ready." }
        })
        if !started {
            signingIn = false
            aiMessage = "FERB couldn't start the sign-in."
        }
    }

    func cancelSignIn() { runner.cancel() }

    func useClaude() async { await use("claude", check: true) }

    var claudeSignIn: SignInStatus? { ai?.signIn("claude") }

    /// Sign in with Claude (owner 2026-10-25): the daemon runs the pinned Claude Code's own sign-in in the browser; the
    /// credential stays with Claude Code in the Keychain. Polling (`refreshAI`) sees it end, then FERB uses Claude.
    func signInWithClaude() async {
        do {
            let status = SignInStatus(try await rpc.call("runtimes.sign_in", params: ["runtime": "claude"]))
            claudeSigningIn = status?.waiting == true
            aiMessage = claudeSigningIn ? "Finish signing in to Claude in the browser window that opened."
                : (status?.why ?? "FERB couldn't start the Claude sign-in.")
        } catch { aiMessage = error.localizedDescription }
        await refreshAI()
    }

    /// The code the sign-in page shows when its own redirect couldn't finish; it goes to the daemon's waiting sign-in
    /// and is never kept here.
    func sendClaudeCode(_ code: String) async {
        let trimmed = code.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        do { _ = try await rpc.call("runtimes.sign_in", params: ["runtime": "claude", "code": trimmed]) }
        catch { aiMessage = error.localizedDescription }
    }

    func cancelClaudeSignIn() async {
        _ = try? await rpc.call("runtimes.sign_in", params: ["runtime": "claude", "cancel": true])
        claudeSigningIn = false
        aiMessage = nil
        await refreshAI()
    }

    /// The owner's click when the browser didn't open by itself.
    func openClaudeSignInPage() {
        if let url = claudeSignIn?.url { system.open(url) }
    }

    func saveAPIKey(_ provider: APIProvider, key: String) async {
        guard APIKeyEntry.isStorable(key) else {
            aiMessage = "That doesn't look like an API key (it has spaces, quotes or other characters)."
            return
        }
        aiMessage = "Saving the key in your Keychain…"
        guard await keys.store(provider: provider, key: key) else {
            aiMessage = "The key wasn't saved in your Keychain."
            return
        }
        await use(provider.rawValue, check: true)
    }

    private func use(_ runtime: String, check: Bool) async {
        let name = AISnapshot.label(runtime)
        do {
            _ = try await rpc.call("runtimes.use", params: ["runtime": runtime])
            if check {
                aiMessage = "Checking that \(name) answers…"
                let reply = try await rpc.call("runtimes.check", params: ["runtime": runtime], wait: 60)
                aiMessage = reply["ok"] as? Bool == true ? "\(name) answered. FERB uses it now."
                    : "\(name) didn't answer: \(reply["detail"] as? String ?? "no answer")"
            } else {
                aiMessage = "FERB uses \(name) now."
            }
        } catch { aiMessage = error.localizedDescription }
        await refreshAI()
    }

    // MARK: Permissions

    func refreshPermissions(probeHearing: Bool = false) async {
        do {
            let reply = try await rpc.call("setup.permissions", params: probeHearing ? ["hearing": true] : [:],
                                           wait: probeHearing ? 30 : 10)
            if var snapshot = PermissionSnapshot(reply: reply) {
                if probeHearing { hearingRow = snapshot.rows.first { $0.id == "hearing" } }
                snapshot.rows.removeAll { $0.id == "hearing" }       // shown from hearingRow, probed on demand
                permissions = snapshot
            }
            clearNotAnswering()
        } catch { message = Self.notAnswering }
        let notifications = await app.notifications()
        let microphone = app.microphone()
        appRows = [
            PermissionRow(id: "notifications", status: notifications, process: "app", required: false,
                          pane: permissions?.appPanes["notifications"], requestable: notifications == .notDetermined),
            PermissionRow(id: "microphone", status: microphone, process: "app", required: false,
                          pane: permissions?.appPanes[app.microphoneDeniedPane], requestable: microphone == .notDetermined)]
    }

    /// The owner's Grant on one row: macOS's own request when there is one (from the daemon for its grants, from FERB.app
    /// for its own), then that pane; Full Disk Access also shows the agent app in Finder to drag into the list.
    func grant(_ row: PermissionRow) async {
        guard row.blocked.isEmpty else { return }
        UserDefaults.standard.set(SetupStep.permissions.rawValue, forKey: Self.resumeKey)
        switch row.id {
        case "notifications":
            if row.status == .notDetermined { _ = await app.requestNotifications() }
            else if let pane = row.pane { system.open(pane) }
        case "microphone":
            if row.status == .notDetermined { _ = await app.requestMicrophone() }
            else if let pane = row.pane { system.open(pane) }
        case "hearing":
            if row.status == .notDetermined {
                _ = try? await rpc.call("setup.permission_request", params: ["permission": "hearing"], wait: 120)
            } else if let pane = row.pane { system.open(pane) }
            await refreshPermissions(probeHearing: true)
            return
        default:
            if row.requestable {
                _ = try? await rpc.call("setup.permission_request", params: ["permission": row.id], wait: 120)
            }
            if row.id == "full_disk_access", let target = permissions?.grantTarget, !target.isEmpty {
                system.reveal(target)
            }
            if let pane = row.pane { system.open(pane) }
            if row.restartToCheck { askedRestart.insert(row.id) }
        }
        await refreshPermissions()
    }

    /// Automation for one running app: macOS asks once (through the daemon, so the grant is the daemon's); after a No it
    /// never asks again, so the pane opens instead.
    func allowAutomation(_ target: AutomationApp) async {
        if target.status == .denied {
            if let pane = permissions?.automationPane { system.open(pane) }
            return
        }
        _ = try? await rpc.call("automation.request", params: ["bundle_ids": [target.bundleID]], wait: 120)
        await refreshPermissions()
    }

    func restartService() async {
        restarting = true
        message = "Restarting FERB's background service…"
        let ok = await service.restart()
        restarting = false
        message = ok ? nil : "FERB's background service couldn't be restarted."
        if ok { askedRestart.removeAll() }
        await refreshPermissions()
    }

    /// After a quiet update: whether a required grant that can be granted now is missing.
    func needsPermissions() async -> Bool {
        await refreshPermissions()
        return permissions?.rows.contains { Self.askedAtSetup.contains($0.id) && $0.status != .granted && $0.blocked.isEmpty } ?? false
    }

    /// Whether setup was finished (by this window or `ferb setup`), asked of the daemon at launch.
    func setupDone(attempts: Int = 3, pause: Duration = .seconds(2)) async -> Bool {
        for attempt in 0..<attempts {
            if let reply = try? await rpc.call("setup.permissions", params: [:]),
               let snapshot = PermissionSnapshot(reply: reply) {
                return snapshot.setupDone
            }
            if attempt + 1 < attempts { try? await Task.sleep(for: pause) }
        }
        return true                 // FERB isn't answering: never nag at launch; its menu says it isn't running
    }

    /// Owner decision 6 (2026-10-25): a launch that shows no setup window (an install set up before this window existed)
    /// asks macOS once for FERB.app's Notifications while macOS hasn't asked; macOS shows its own Allow prompt, which
    /// takes no focus. Remembered in FERB.app's defaults (`FERBAskedNotifications`), so it is asked once. True when it
    /// asked.
    func askForNotificationsOnce(defaults: UserDefaults) async -> Bool {
        guard !defaults.bool(forKey: "FERBAskedNotifications"), await app.notifications() == .notDetermined else {
            return false
        }
        defaults.set(true, forKey: "FERBAskedNotifications")
        _ = await app.requestNotifications()
        return true
    }

    // MARK: Browsers

    func refreshBrowsers() async {
        do {
            browsers = BrowserSnapshot(reply: try await rpc.call("setup.browsers", params: [:]))
            clearNotAnswering()
        } catch { message = Self.notAnswering }
    }

    func addExtension(_ profile: BrowserProfileRow) async {
        do {
            _ = try await rpc.call("setup.browser_open", params: ["browser": profile.browser, "folder": profile.folder])
        } catch { message = error.localizedDescription }
    }

    // MARK: Getting to know you

    func refreshPersona() async {
        let params: [String: Any] = persona == .loading ? [:] : ["status": true]
        do {
            persona = PersonaState(reply: try await rpc.call("setup.persona", params: params))
            clearNotAnswering()
        } catch { message = Self.notAnswering }
    }

    func savePersona() async {
        guard case .pending(let id, _, _, true) = persona else { return }
        await answerPersona("chat.approve", ["approval_id": id, "granted": true, "choice": 1])
    }

    func editPersona(_ text: String) async {
        guard case .pending(let id, _, _, _) = persona,
              !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        await answerPersona("chat.answer", ["approval_id": id, "text": text])
    }

    func skipPersona() async {
        guard case .pending(let id, _, _, _) = persona else { return }
        await answerPersona("chat.approve", ["approval_id": id, "granted": false])
    }

    private func answerPersona(_ method: String, _ params: [String: Any]) async {
        do {
            _ = try await rpc.call(method, params: params)
            persona = .finished
            personaMessage = nil
        } catch { personaMessage = error.localizedDescription }
    }

    // MARK: Done

    func finish() async {
        _ = try? await rpc.call("setup.finish", params: [:])       // only records it; Done still opens the notch
        stopPolling()
        onFinish?()
    }
}
