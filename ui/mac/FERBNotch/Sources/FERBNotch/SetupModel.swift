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
    /// macOS waits for the owner's OK in System Settings → General → Login Items before it runs the service.
    @Published private(set) var serviceNeedsApproval = false
    @Published private(set) var ai: AISnapshot?
    @Published private(set) var aiMessage: String?
    @Published private(set) var signingIn = false
    @Published private(set) var claudeSigningIn = false
    @Published private(set) var permissions: PermissionSnapshot?
    @Published private(set) var hearingRow: PermissionRow?
    @Published private(set) var appRows: [PermissionRow] = []
    @Published private(set) var askedRestart: Set<String> = []
    /// Rows a Grant asked for: one still off gets "Show FERB in Finder" to drag it into its pane's list (a friend's
    /// newer macOS, 2026-10-27: FERB was missing from "Device Control and Data Access").
    @Published private(set) var askedRows: Set<String> = []
    /// While macOS's notification request waits on its banner; nothing else waits on it (a friend's Allow all stalled).
    @Published private(set) var notificationHint = false
    /// After macOS's own request, the wait before its pane opens, so the pane already lists FERB.
    var requestSettle: Duration = .milliseconds(1500)
    /// Grants whose switch applies only after a restart: the service restarts once when FERB comes back to the front.
    private var restartOnReturn: Set<String> = []
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
    /// The app's registered background service; nil where there is none to register (a development build, tests).
    let background: BackgroundServiceRegistering?
    /// How long the window waits for the service to answer after macOS turned it on.
    var serviceStartWait: Duration = .seconds(30)
    var onFinish: (() -> Void)?
    private var poll: Task<Void, Never>?

    static let notAnswering = "FERB isn't answering right now; this step updates when it's back."

    init(rpc: DaemonRPCTransport, runner: SetupCommandRunning, app: AppPermissionsProviding, system: SystemOpening,
         keys: APIKeyStoring, service: DaemonServicing, background: BackgroundServiceRegistering? = nil) {
        self.rpc = rpc
        self.runner = runner
        self.app = app
        self.system = system
        self.keys = keys
        self.service = service
        self.background = background
    }

    /// What the setup window asks for: what every task needs. The rest are asked when first needed (owner 2026-10-26:
    /// "ask when first needed, like Claude and ChatGPT"); Settings → Permissions lists them all.
    static let askedAtSetup: Set<String> = ["accessibility", "screen_recording", "full_disk_access", "input_monitoring",
                                            "notifications", "microphone", "hearing", "clipboard"]
    /// "Allow all" (owner 2026-10-26: "lets just have them all at the start only with the one allow all"): every setup
    /// row not yet on, one-click popups first; Screen Recording last, since macOS restarts FERB after it (setup reopens
    /// on this page). Automation can't be granted ahead (macOS asks per app at first use).
    /// Notifications late (2026-10-27): its banner is answered whenever the owner gets to it, and nothing waits on it.
    static let allowAllSequence = ["microphone", "hearing", "clipboard", "input_monitoring",
                                   "accessibility", "full_disk_access", "notifications", "screen_recording"]
    /// Answered in macOS's own popup: no wait for a switch afterwards.
    static let answeredInPopup: Set<String> = ["notifications", "microphone", "hearing", "clipboard"]
    static func allowAllOrder(_ rows: [PermissionRow]) -> [String] {
        allowAllSequence.filter { id in rows.contains { $0.id == id && $0.status != .granted && $0.blocked.isEmpty } }
    }
    @Published private(set) var allowingAll = false

    /// Each setup permission in turn: macOS's own request and its pane, then a wait until the row ticks (polled), before
    /// the next. The owner can stop by closing the pane; a row still off after its wait is left for its own Grant.
    func allowAll() async {
        guard !allowingAll else { return }
        allowingAll = true
        defer { allowingAll = false }
        for id in Self.allowAllOrder(setupRows) {
            guard let row = allRows.first(where: { $0.id == id }) else { continue }
            await grant(row)
            if Self.answeredInPopup.contains(id) { continue }
            for _ in 0..<180 {                       // up to 3 minutes for the owner to flip this one switch
                await refreshPermissions()
                if allRows.contains(where: { $0.id == id && $0.status == .granted }) { break }
                try? await Task.sleep(for: .seconds(1))
            }
        }
    }
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

    /// `ferb setup --service [--update]`: the background service's files from this app, no questions; then the app turns
    /// the service on with macOS (`registerService`). On failure its last lines (the reason and its fix) stay on Welcome.
    func runService(completion: @escaping (Bool) -> Void) {
        guard let arguments = AppSetupPlan.serviceArguments(for: serviceAction) else { completion(true); return }
        serviceError = nil
        serviceNeedsApproval = false
        serviceLine = "Starting…"
        serviceRunning = true
        var tail: [String] = []
        let started = runner.run(arguments, line: { [weak self] text in
            self?.serviceLine = text
            tail = Array((tail + [text]).suffix(6))
        }, done: { [weak self] status in
            guard let self else { return }
            if status == 0 && self.background == nil {      // nothing to register (a development build, tests)
                self.serviceRunning = false
                self.serviceAction = .none
                completion(true)
            } else if status == 0 {
                Task { @MainActor in
                    let on = await self.registerService()
                    self.serviceRunning = false
                    if on { self.serviceAction = .none }
                    completion(on)
                }
            } else {
                self.serviceRunning = false
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

    /// The app's background service turned on with macOS (registering again is harmless), then a wait until FERB
    /// answers. macOS's "needs your OK", a missing definition or a refusal stays on Welcome in the app's words.
    func registerService() async -> Bool {
        guard let background else { return true }
        serviceLine = "Turning on FERB's background service…"
        do {
            try background.register()
        } catch {
            if background.status != .enabled && background.status != .requiresApproval {
                serviceError = ServicePlan.failed(error.localizedDescription)
                return false
            }
        }
        if let problem = ServicePlan.problem(background.status) {
            serviceNeedsApproval = background.status == .requiresApproval
            serviceError = problem
            return false
        }
        let end = ContinuousClock.now + serviceStartWait
        while ContinuousClock.now < end {
            if (try? await rpc.call("ping", params: [:])) != nil { return true }
            try? await Task.sleep(for: .milliseconds(500))
        }
        serviceError = ServicePlan.notStarted
        return false
    }

    /// System Settings → General → Login Items, where the owner allows FERB's background service.
    func openLoginItems() { background?.openLoginItems() }

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
            PermissionRow(id: "notifications", status: notifications, process: "app", required: true,
                          pane: permissions?.appPanes["notifications"], requestable: notifications == .notDetermined),
            PermissionRow(id: "microphone", status: microphone, process: "app", required: true,
                          pane: permissions?.appPanes[app.microphoneDeniedPane], requestable: microphone == .notDetermined)]
    }

    /// The owner's Grant on one row: macOS's own request when there is one (from the daemon for its grants, from FERB.app
    /// for its own), then that pane; Full Disk Access also shows the agent app in Finder to drag into the list.
    func grant(_ row: PermissionRow) async {
        guard row.blocked.isEmpty else { return }
        UserDefaults.standard.set(SetupStep.permissions.rawValue, forKey: Self.resumeKey)
        switch row.id {
        case "notifications":
            if row.status == .notDetermined {
                notificationHint = true                  // macOS's banner waits at the top right; nothing waits on it
                Task { [weak self] in
                    _ = await self?.app.requestNotifications()
                    self?.notificationHint = false
                    await self?.refreshPermissions()
                }
                return
            } else if let pane = row.pane { system.open(pane) }
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
                try? await Task.sleep(for: requestSettle)
            }
            if row.id == "full_disk_access", let target = permissions?.grantTarget, !target.isEmpty {
                system.reveal(target)
            }
            if let pane = row.pane { system.open(pane) }
            askedRows.insert(row.id)
            if row.restartToCheck {
                askedRestart.insert(row.id)
                restartOnReturn.insert(row.id)
            }
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

    /// FERB came to the front again, the owner back from System Settings (2026-10-27: a friend's Screen Recording and
    /// Input Monitoring never ticked): a switch that applies only after a restart is checked by restarting the service,
    /// once per Grant, and never while a task runs (the "Restart FERB to check" button stays for that).
    func cameBack() async {
        let pending = restartOnReturn.filter { id in allRows.contains { $0.id == id && $0.status != .granted } }
        restartOnReturn.removeAll()
        guard !pending.isEmpty, !restarting else { return }
        if let status = try? await rpc.call("chat.status", params: ["peek": true]),
           let running = status["running"] as? [[String: Any]], !running.isEmpty { return }
        await restartService()
    }

    /// The agent app in Finder, to drag into a pane's list that doesn't show FERB.
    func showAgentApp() {
        if let target = permissions?.grantTarget, !target.isEmpty { system.reveal(target) }
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

    /// Pairing happens on the owner's click (owner 2026-10-27: "a pair all which opens all pairs instantly and closes all
    /// too", and "single per profile too"): each profile's own pairing page opens and closes itself.
    @Published private(set) var pairing = false

    func pairProfile(_ profile: BrowserProfileRow) async {
        guard !pairing else { return }
        pairing = true
        defer { pairing = false }
        do {
            let reply = try await rpc.call("setup.browser_pair", params: ["browser": profile.browser,
                                                                          "folder": profile.folder], wait: 90)
            message = reply["paired"] as? Bool == true ? nil : (reply["why"] as? String ?? "That profile didn't pair.")
        } catch { message = error.localizedDescription }
        await refreshBrowsers()
    }

    func pairAll() async {
        guard !pairing else { return }
        pairing = true
        defer { pairing = false }
        do {
            let reply = try await rpc.call("setup.browsers_pair_all", params: [:], wait: 90)
            let results = reply["results"] as? [[String: Any]] ?? []
            let failed = results.filter { $0["paired"] as? Bool != true }.map { "\($0["name"] as? String ?? "a profile")" }
            message = failed.isEmpty ? nil : "Not paired: " + failed.joined(separator: ", ") + ". Is FERB turned on there?"
        } catch { message = error.localizedDescription }
        await refreshBrowsers()
    }

    // MARK: Getting to know you

    func refreshPersona() async {
        let params: [String: Any] = persona == .loading ? [:] : ["status": true]
        do {
            persona = PersonaState(reply: try await rpc.call("setup.persona", params: params))
            clearNotAnswering()
        } catch { message = Self.notAnswering }
    }

    // MARK: Done

    /// Done: the daemon records setup as done only when every permission is granted, FERB.app's own included (it
    /// reads its own; FERB.app reports these); Done still opens the notch, and launch shows the window again otherwise.
    func finish() async {
        let own = Dictionary(uniqueKeysWithValues: appRows.map { ($0.id, $0.status.rawValue) })
        _ = try? await rpc.call("setup.finish", params: ["app": own])
        stopPolling()
        onFinish?()
    }
}
