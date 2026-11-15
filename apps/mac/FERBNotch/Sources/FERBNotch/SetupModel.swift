import AppKit
import NotchCore

/// Setup's state and actions (L34 C), for the tour and Settings' pages (there is no setup window, APP-685). Every fact
/// comes from the daemon's setup RPCs or FERB.app's own permission APIs; nothing here decides a grant or a choice.
@MainActor
final class SetupModel: ObservableObject {
    @Published var step: SetupStep = .welcome
    @Published var serviceAction: AppSetupAction = .none
    @Published private(set) var serviceRunning = false
    @Published private(set) var serviceLine = ""
    @Published private(set) var serviceError: String?
    /// The service is switched off in System Settings → General → Login Items → Allow in the Background: macOS runs
    /// it once the owner switches FERB on there.
    @Published private(set) var serviceNeedsApproval = false
    @Published private(set) var ai: AISnapshot?
    @Published private(set) var aiMessage: String?
    /// The background service didn't answer the page's last read: the page says so and dims what it shows (APP-884).
    @Published private(set) var daemonSilent = false
    /// When this page last changed the AI itself: a change event right after it is its own, not one made elsewhere.
    private var ownAIChange = Date.distantPast
    @Published private(set) var signingIn = false
    @Published private(set) var claudeSigningIn = false
    @Published private(set) var permissions: PermissionSnapshot?
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
    /// The grant being waited on now and what that wait is doing (asking macOS, opening Settings, waiting for the
    /// switch, checking, restarting): the tour shows it live (APP-665, APP-679). Nil when no grant is waited on.
    @Published private(set) var grantWait: GrantWait?
    /// The rows a grant's wait reads through a fresh process too (`setup.permissions` `fresh`, APP-662): the running
    /// daemon sees these switches only after a restart.
    private var freshReads: Set<String> = []
    @Published private(set) var browsers: BrowserSnapshot?
    @Published private(set) var persona: PersonaState = .loading
    /// FERB's own Google account (APP-1, optional): as set now, and the profiles found for the address typed.
    @Published private(set) var identity: IdentityState?
    @Published var identityAddress = ""
    @Published private(set) var identityLine: String?
    @Published private(set) var identityBusy = false
    @Published private(set) var personaMessage: String?
    @Published var message: String?

    let rpc: DaemonRPCTransport
    let runner: SetupCommandRunning
    let app: AppPermissionsProviding
    let system: SystemOpening
    let keys: APIKeyStoring
    let service: DaemonServicing
    /// macOS's side of the background service: the Rust build's own registration (DMN-593), the Python build's 2026-11
    /// registration to retire (DMN-513), and Login Items' switch; nil in a development build and tests that don't fake it.
    let background: BackgroundServiceManaging?
    private var poll: Task<Void, Never>?

    static let notAnswering = "FERB isn't answering right now; this step updates when it's back."

    init(rpc: DaemonRPCTransport, runner: SetupCommandRunning, app: AppPermissionsProviding, system: SystemOpening,
         keys: APIKeyStoring, service: DaemonServicing, background: BackgroundServiceManaging? = nil) {
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
                                            "notifications", "microphone", "clipboard"]
    /// Stable order for the required setup grants. Quick grouping uses each row's restart requirement.
    /// Automation is requested per app on first use; Notifications never blocks on its unanswered banner.
    static let allowAllSequence = ["microphone", "clipboard", "input_monitoring",
                                   "accessibility", "full_disk_access", "notifications", "screen_recording"]
    /// Answered in macOS's own popup: no wait for a switch afterwards.
    static let answeredInPopup: Set<String> = ["notifications", "microphone", "clipboard"]
    static func allowAllOrder(_ rows: [PermissionRow]) -> [String] {
        allowAllSequence.filter { id in rows.contains { $0.id == id && $0.status != .granted && $0.blocked.isEmpty } }
    }
    /// Batch only the permissions whose grant does not need a restart; the rest are individual visits.
    static func quickPermissionOrder(_ rows: [PermissionRow]) -> [String] {
        allowAllOrder(rows).filter { id in rows.contains { $0.id == id && !$0.restartToCheck } }
    }
    @Published private(set) var allowingAll = false

    /// Each setup permission in turn: macOS's own request and its pane, then a wait until the row ticks (polled), before
    /// the next. The owner can stop by closing the pane; a row still off after its wait is left for its own Grant.
    func allowQuickPermissions() async {
        guard !allowingAll else { return }
        allowingAll = true
        defer { allowingAll = false }
        for id in Self.quickPermissionOrder(setupRows) {
            guard let row = allRows.first(where: { $0.id == id }) else { continue }
            if Self.answeredInPopup.contains(id) { await grant(row); continue }
            _ = await grantAndConfirm(row, polls: 120)    // up to 3 minutes for the owner to flip this one switch
        }
    }

    /// How often a grant's row is read again while the owner flips its switch (kb area app: ~1–1.5 s).
    var grantPoll: Duration = .milliseconds(1500)
    /// The app in front (its bundle id): a grant's wait notices the owner leaving System Settings.
    var frontmostApp: () -> String? = { NSWorkspace.shared.frontmostApplication?.bundleIdentifier }

    /// One row's grant (`grant`: macOS's own request where there is one, then its pane), then its row read again
    /// every `grantPoll` until the system reports it on: true only then, never inferred. `asked` runs once the request
    /// and the pane are up (the intro docks its orb there). A row macOS asked about in its own popup that comes back
    /// denied returns false at once (its pane is the way on); otherwise it waits until on, `polls` reads at most, or
    /// the calling task is cancelled.
    /// A switch that reaches the daemon only after a restart (Screen Recording, Input Monitoring, Full Disk Access) is
    /// checked by restarting the service each time the owner leaves System Settings while it still reads off: FERB.app
    /// is an accessory app that never comes to the front by itself, so leaving Settings is the signal (owner's live run,
    /// 2026-11-01). A daemon that stops answering for `serviceStartAfter` reads (macOS's "Quit & Reopen" can end it
    /// with a clean exit, which launchd doesn't restart) is started again, once per silence.
    /// While it waits, the row is also read by a fresh process (APP-662): the moment that sees the switch on while the
    /// daemon still reads it off, the service restarts at once (once per wait, never while a task runs), so the row
    /// ticks within a poll and a restart instead of waiting for the owner to leave System Settings.
    /// Each step of the wait is published in `grantWait` as it happens (APP-679), so nothing here runs unseen.
    func grantAndConfirm(_ row: PermissionRow, polls limit: Int? = nil, asked: (() -> Void)? = nil) async -> Bool {
        let undecided = row.status == .notDetermined
        if row.restartToCheck { freshReads.insert(row.id) }
        grantWait = GrantWait(id: row.id, step: row.requestable ? .asking : .opening, at: Date())
        defer {
            freshReads.remove(row.id)
            if grantWait?.id == row.id { grantWait = nil }
        }
        await grant(row) { [weak self] step in self?.moveGrant(step) }
        // Already on once macOS answered: no pane, no dock beside Settings, nothing for the owner to do (APP-697).
        if allRows.first(where: { $0.id == row.id })?.status == .granted { return true }
        moveGrant(.waitingForSwitch)
        asked?()
        var polls = 0
        var inSettings = false
        var unanswered = 0
        var restartedForSwitch = false
        while !Task.isCancelled {
            if let now = allRows.first(where: { $0.id == row.id }) {
                if now.status == .granted { return true }
                if undecided && Self.answeredInPopup.contains(row.id) && now.status == .denied { return false }
            }
            if let limit, polls >= limit { return false }
            do { try await Task.sleep(for: grantPoll) } catch { return false }
            polls += 1
            let front = frontmostApp() == SettingsAnchor.bundleID
            if inSettings && !front, allRows.first(where: { $0.id == row.id })?.restartToCheck == true {
                await restartUnlessWorking(seenOn: false)
            }
            inSettings = front
            // A silence already being started again stays shown as that until FERB answers.
            if grantWait?.step != .startingService { moveGrant(.checking) }
            if await refreshPermissions() {
                unanswered = 0
                if !restartedForSwitch, allRows.first(where: { $0.id == row.id })?.restartNeeded == true {
                    restartedForSwitch = true
                    await restartUnlessWorking(seenOn: true)
                }
                if allRows.first(where: { $0.id == row.id })?.status != .granted { moveGrant(.waitingForSwitch) }
            } else {
                unanswered += 1
                if unanswered == serviceStartAfter {
                    moveGrant(.startingService)
                    _ = await service.start()
                    if let why = await serviceSpawnFailure(within: serviceCheckAfterStart) { _ = await repairService(why) }
                } else if grantWait?.step != .startingService {
                    moveGrant(.waitingForSwitch)
                }
            }
        }
        return false
    }

    /// The waited-on grant moves to `step` (nothing waits: nothing to show).
    private func moveGrant(_ step: GrantWait.Step) { grantWait?.move(to: step, at: Date()) }

    /// A resumed tour is waiting for the background service to answer, starting it if it stays silent.
    @Published var waitingForService = false

    /// FERB.app is setting its background service up: an install or update not yet run (a first run before Meet)
    /// and not failed, its command running, a restart, or a wait that starts it again. A daemon that doesn't answer
    /// meanwhile is expected, so the menu bar says "Setting up FERB…", never "isn't running" (APP-698); a failed
    /// install shows its reason in the tour and the menu bar's error again.
    var settingUpService: Bool {
        serviceRunning || restarting || waitingForService || grantWait?.step == .startingService
            || (serviceAction != .none && serviceError == nil)
    }

    /// Reads without an answer before a grant's wait starts the background service (about 4.5 s at `grantPoll`): a
    /// daemon that is only slow is never killed, `start` leaves a running one alone.
    var serviceStartAfter = 3

    /// How long after a restart or start launchd's state is read for a job it can't start (its attempt comes at once;
    /// a failure repeats every ThrottleInterval, 5 s).
    var serviceCheckAfterStart: Duration = .seconds(3)

    /// The background service answers `ping`.
    func serviceAnswers() async -> Bool { (try? await rpc.call("ping", params: [:])) != nil }
    var setupRows: [PermissionRow] { allRows.filter { Self.askedAtSetup.contains($0.id) } }

    var allRows: [PermissionRow] { (permissions?.rows ?? []) + appRows }

    var offersRestart: Bool {
        allRows.contains { ($0.restartNeeded || askedRestart.contains($0.id)) && $0.status != .granted }
    }

    var showsFullDiskHint: Bool {
        askedRestart.contains("full_disk_access")
            && allRows.contains { $0.id == "full_disk_access" && $0.status != .granted }
    }

    /// This build registers its background service itself (SMAppService, DMN-593: the Rust build, InAppService); the
    /// Python build runs the LaunchAgent (DMN-513).
    var appService = false
    /// This build is signed without a Team ID (a developer's ad-hoc Rust build): its registration is made afresh when
    /// setup runs, since its launch constraint names one exact build (AppServicePlan.registersAfresh). A Developer ID
    /// build keeps its registration across updates.
    var registersAfresh = false
    /// How long FERB.app waits for the daemon after macOS turned its registration on.
    var serviceStartWait: Duration = .seconds(30)

    /// `ferb setup --service [--update]`, no questions. The Rust build (`appService`, DMN-593) runs it with
    /// `--app-service` (FERB's files set up, any LaunchAgent retired, nothing loaded), then registers its own service and
    /// waits for FERB; that registration is the only way it runs the service. macOS's refusal, or a job launchd can't
    /// start even after one fresh registration (DMN-594), leaves the reason and the Login Items guidance in
    /// `serviceError`; the owner's Login Items switch is guided as one visit (`serviceNeedsApproval`). The Python build
    /// writes and loads its LaunchAgent (DMN-513), unregistering a registration a 2026-11 build left first, once. Any
    /// other failure leaves its last lines (the reason and its fix) in `serviceError`, which the tour's service line
    /// shows with Try again (APP-685).
    func runService(completion: @escaping (Bool) -> Void) {
        let inApp = appService && background != nil
        guard let arguments = AppSetupPlan.serviceArguments(for: serviceAction, appService: inApp) else {
            completion(true)
            return
        }
        serviceError = nil
        serviceNeedsApproval = false
        serviceLine = "Starting…"
        serviceRunning = true
        guard !inApp, let background, background.registration.listed else {
            startServiceCommand(arguments, inApp: inApp, completion: completion)
            return
        }
        Task { @MainActor in
            self.serviceLine = "Turning off the background service an earlier FERB registered…"
            try? await background.unregister()          // `ferb setup` boots out whatever still runs under the label
            self.startServiceCommand(arguments, inApp: false, completion: completion)
        }
    }

    /// FERB.app's own registration turned on (DMN-593), then a wait until FERB answers. A build without a Team ID
    /// registers afresh (`registersAfresh`); a Developer ID build's registration is kept.
    private func turnOnAppService(completion: @escaping (Bool) -> Void) async {
        guard let background else { return completion(true) }
        serviceRunning = true
        serviceLine = "Turning on FERB's background service…"
        if registersAfresh && (background.registration.listed || background.status.listed) {
            try? await background.unregister()
        }
        let why = await registerAndWait(background)
        serviceRunning = false
        if let why {
            serviceError = why
            completion(false)
        } else {
            serviceAction = .none
            completion(true)
        }
    }

    /// `register()` (already registered counts as done), then a wait until FERB answers: nil then, else what the owner
    /// reads. The owner's Login Items switch is the Login Items guidance (`serviceNeedsApproval`, ServicePlan.approval);
    /// anything else is macOS's or launchd's reason with how to allow it (AppServicePlan.refusal). Never anything around
    /// the registration.
    private func registerAndWait(_ background: BackgroundServiceManaging) async -> String? {
        var failure: String?
        var denied = false
        do { try background.register() } catch is AppServicePlan.DeniedByUser { denied = true } catch {
            failure = error.localizedDescription
        }
        switch AppServicePlan.afterRegister(error: failure, deniedByUser: denied, status: background.status) {
        case .approval:
            serviceNeedsApproval = true
            return ServicePlan.approval
        case .refused(let why):
            NSLog("FERB service: macOS refused the registration: %@", why)
            return AppServicePlan.refusal(why)
        case .waitForFERB:
            return await waitForAppService(background)
        }
    }

    /// How often a wait for the background service reads it (tests shorten it).
    var serviceWaitPoll: Duration = .milliseconds(500)

    /// Waits until FERB.app's registered service answers (DMN-593): nil then, else what the owner reads. When launchd
    /// says it can't start the job (`spawnFailure`; live 2026-11-14: the registration had been tied to the Background
    /// Task Management record of the user LaunchAgent it replaced, and once macOS dropped that record every start failed
    /// with EX_CONFIG), the job is registered once more at once, a fresh record, instead of waiting out
    /// `serviceStartWait` (DMN-594); failing again, launchd's words are the reason.
    private func waitForAppService(_ background: BackgroundServiceManaging) async -> String? {
        var again = false
        let end = ContinuousClock.now + serviceStartWait
        while ContinuousClock.now < end {
            if await serviceAnswers() { return nil }
            if let why = background.spawnFailure() {
                if again {
                    NSLog("FERB service: %@ after registering it again", why)
                    return AppServicePlan.refusal(why)
                }
                again = true
                serviceLine = "Registering FERB's background service again…"
                NSLog("FERB service: %@; registering it again", why)
                try? await background.unregister()
                do { try background.register() } catch is AppServicePlan.DeniedByUser {
                    serviceNeedsApproval = true
                    return ServicePlan.approval
                } catch { return AppServicePlan.refusal(why) }
                try? await Task.sleep(for: serviceWaitPoll * 2)    // launchd's first attempt for the new job
                continue
            }
            try? await Task.sleep(for: serviceWaitPoll)
        }
        return AppServicePlan.refusal(AppServicePlan.notStarted)
    }

    /// After FERB.app restarted or started the background service (a grant's restart-to-check, a silence, the menu's
    /// Restart): launchd's own word that it can't start the job, read for `within` while FERB doesn't answer; nil once
    /// FERB answers or launchd says nothing of the kind. Restarting goes through `launchctl kickstart`, which reports
    /// success before launchd's attempt (live 2026-11-14: kickstart -k returned, then every respawn failed and the
    /// tour waited on Input Monitoring for good).
    func serviceSpawnFailure(within: Duration) async -> String? {
        guard let background else { return nil }
        let end = ContinuousClock.now + within
        while ContinuousClock.now < end {
            try? await Task.sleep(for: serviceWaitPoll)        // the old process may still answer as it exits
            if await serviceAnswers() { return nil }
            if let why = background.spawnFailure() { return why }
        }
        return nil
    }

    /// launchd can't start FERB's background service. The Rust build's registration is made again once (DMN-594) and,
    /// if launchd still can't start it, the owner reads why and how to allow it (`serviceError`, DMN-593); nothing runs
    /// around it. The Python build installs its LaunchAgent again (DMN-513). True when FERB answers afterwards.
    func repairService(_ why: String) async -> Bool {
        serviceRunning = true
        if appService, let background {
            serviceError = nil
            serviceNeedsApproval = false
            NSLog("FERB service: %@", why)
            // the wait sees launchd's failure and registers once more; a registration macOS dropped is made again
            let listed = background.registration.listed || background.status.listed
            let failure = listed ? await waitForAppService(background) : await registerAndWait(background)
            serviceRunning = false
            serviceError = failure
            return failure == nil
        }
        serviceLine = "launchd couldn't start FERB's background service (\(why)), so FERB sets it up again."
        NSLog("FERB service: %@", serviceLine)
        return await withCheckedContinuation { (done: CheckedContinuation<Bool, Never>) in
            Task { @MainActor in
                self.startServiceCommand(["setup", "--service", "--update"], inApp: false) { done.resume(returning: $0) }
            }
        }
    }

    private func startServiceCommand(_ arguments: [String], inApp: Bool, completion: @escaping (Bool) -> Void) {
        var tail: [String] = []
        let started = runner.run(arguments, line: { [weak self] text in
            self?.serviceLine = text
            tail = Array((tail + [text]).suffix(6))
        }, done: { [weak self] status in
            guard let self else { return }
            self.serviceRunning = false
            switch status {
            case 0 where inApp:
                Task { @MainActor in await self.turnOnAppService(completion: completion) }
            case 0:
                self.serviceAction = .none
                completion(true)
            case ServicePlan.switchedOffStatus:
                self.serviceNeedsApproval = true
                self.serviceError = ServicePlan.approval
                completion(false)
            default:
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

    /// System Settings → General → Login Items, where the owner allows FERB's background service.
    func openLoginItems() { background?.openLoginItems() }

    // MARK: Polling

    /// Re-reads `step`'s facts every `every` while a Settings page shows them (Settings → Permissions every 2 s,
    /// Browsers every 3 s, APP-883). While Permissions polls, every off row whose switch reaches FERB only after a
    /// restart is also read by a fresh process, so a switch already on in macOS says so (APP-884).
    func startPolling(_ step: SetupStep, every: Duration = .seconds(2)) {
        self.step = step
        pagePollsPermissions = step == .permissions
        poll?.cancel()
        poll = Task { [weak self] in
            while !Task.isCancelled {
                await self?.refreshCurrentStep()
                do { try await Task.sleep(for: every) } catch { return }
            }
        }
    }

    func stopPolling() {
        poll?.cancel()
        poll = nil
        pagePollsPermissions = false
    }

    /// Settings → Permissions is shown and polling.
    private var pagePollsPermissions = false

    func refreshCurrentStep() async {
        switch step {
        case .ai: await refreshAI()
        case .permissions: await refreshPermissions()
        case .browsers: await refreshBrowsers()
        case .persona: if persona == .loading || persona == .scanning { await refreshPersona() }
        case .identity: if !identityBusy { await refreshIdentity() }
        case .welcome, .done: break
        }
    }

    func clearNotAnswering() { if message == Self.notAnswering { message = nil } }

    // MARK: Choose your AI

    /// `changedElsewhere`: a change event or FERB coming back to the front; when it shows the AI changed, the page's
    /// last line (about the old state) goes (APP-882).
    func refreshAI(changedElsewhere: Bool = false) async {
        do {
            let fresh = AISnapshot(reply: try await rpc.call("runtimes.status", params: [:]))
            if changedElsewhere, fresh != ai, ai != nil, Date().timeIntervalSince(ownAIChange) > 5 { aiMessage = nil }
            ai = fresh
            daemonSilent = false
            clearNotAnswering()
        } catch {
            message = Self.notAnswering
            daemonSilent = true
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
    /// A key already in the Keychain, used without entering it again (RUN-429).
    func useAPIKey(_ provider: APIProvider) async { await use(provider.rawValue, check: true) }
    func useChatGPT() async { await use("codex", check: true) }

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

    /// The models a connection can use (RUN-429), nil when the daemon didn't answer.
    func models(_ runtime: String) async -> ModelList? {
        guard let reply = try? await rpc.call("runtimes.models", params: ["runtime": runtime], wait: 30) else { return nil }
        return ModelList(reply: reply)
    }

    /// The owner's model and effort for a connection, or for one role on it (`role`; "" clears the role's own; effort ""
    /// is the model's default), saved by one Use (APP-881). The daemon checks them first and keeps the previous ones
    /// when they fail; the line says which, in the provider's words, and says "FERB uses …" only when a role runs on
    /// that connection (APP-882).
    func setModel(_ runtime: String, _ chosen: String, effort: String, role: String? = nil,
                  connectionModel: String = "") async -> String {
        var params: [String: Any] = ["runtime": runtime, "model": chosen, "effort": effort]
        if let role { params["role"] = role }
        ownAIChange = Date()
        do {
            let reply = try await rpc.call("runtimes.model", params: params, wait: 90)
            await refreshAI()
            if reply["ok"] as? Bool == true {
                return ModelSaveLine.saved(model: chosen, effort: effort, connectionModel: connectionModel,
                                           runtime: runtime, role: role, running: ai?.runs(runtime, role: role) == true)
            }
            let tried = [chosen, effort.isEmpty ? "" : "\(effort) effort"].filter { !$0.isEmpty }.joined(separator: " at ")
            return "\(tried.isEmpty ? "That" : tried) didn't work, so FERB keeps what it had: "
                + (reply["detail"] as? String ?? "no answer")
        } catch { return error.localizedDescription }
    }

    /// FERB uses `runtime` from its next call. `check`: one tiny call first; when it fails, the service in use stays
    /// (APP-792, owner 2026-11-08) and the line says why.
    private func use(_ runtime: String, check: Bool) async {
        let name = AISnapshot.label(runtime)
        ownAIChange = Date()
        do {
            if check {
                aiMessage = "Checking that \(name) answers…"
                let reply = try await rpc.call("runtimes.check", params: ["runtime": runtime], wait: 60)
                guard reply["ok"] as? Bool == true else {
                    aiMessage = "\(name) didn't answer, so FERB keeps the service it uses: "
                        + (reply["detail"] as? String ?? "no answer")
                    await refreshAI()
                    return
                }
            }
            _ = try await rpc.call("runtimes.use", params: ["runtime": runtime])
            aiMessage = check ? "\(name) answered. FERB uses it now." : "FERB uses \(name) now."
        } catch { aiMessage = error.localizedDescription }
        await refreshAI()
    }

    /// The owner's Remove key: the Keychain item goes (FERB.app removes it, as it stored it); the card goes with it.
    func removeAPIKey(_ provider: APIProvider) async {
        aiMessage = await keys.remove(provider: provider) ? "Your \(provider.label) key is removed from your Keychain."
            : "The \(provider.label) key wasn't removed from your Keychain."
        await refreshAI()
    }

    // MARK: Permissions

    /// True when the daemon answered.
    @discardableResult
    func refreshPermissions() async -> Bool {
        var answered = false
        do {
            var fresh = freshReads
            if pagePollsPermissions {
                fresh.formUnion((permissions?.rows ?? []).filter { $0.restartToCheck && $0.status != .granted }.map(\.id))
            }
            let params: [String: Any] = fresh.isEmpty ? [:] : ["fresh": fresh.sorted()]
            let reply = try await rpc.call("setup.permissions", params: params, wait: 10)
            if let snapshot = PermissionSnapshot(reply: reply) { permissions = snapshot }
            clearNotAnswering()
            daemonSilent = false
            answered = true
        } catch {
            message = Self.notAnswering
            daemonSilent = true
        }
        let notifications = await app.notifications()
        let microphone = app.microphone()
        appRows = [
            PermissionRow(id: "notifications", status: notifications, process: "app", required: true,
                          pane: permissions?.appPanes["notifications"], requestable: notifications == .notDetermined),
            PermissionRow(id: "microphone", status: microphone, process: "app", required: true,
                          pane: permissions?.appPanes[app.microphoneDeniedPane], requestable: microphone == .notDetermined)]
        return answered
    }

    /// The owner's Grant on one row: macOS's own request when there is one (from the daemon for its grants, from FERB.app
    /// for its own), then that pane; Full Disk Access also shows the agent app in Finder to drag into the list.
    /// `step` hears each part as it starts (macOS's question, Settings opening) for a grant's live line (APP-679).
    func grant(_ row: PermissionRow, step: ((GrantWait.Step) -> Void)? = nil) async {
        guard row.blocked.isEmpty else { return }
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
            if row.status == .notDetermined { step?(.asking); _ = await app.requestMicrophone() }
            else if let pane = row.pane { step?(.opening); system.open(pane) }
        default:
            if row.requestable {
                step?(.asking)
                let reply = try? await rpc.call("setup.permission_request", params: ["permission": row.id], wait: 120)
                // macOS answers that it is already on (Input Monitoring reads on once Accessibility is, live
                // 2026-11-04): System Settings has nothing to show, so its pane isn't opened (APP-697).
                if GrantStatus(daemon: reply?["status"]) == .granted {
                    await refreshPermissions()
                    return
                }
                try? await Task.sleep(for: requestSettle)
            }
            step?(.opening)
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
        guard !pending.isEmpty else { return }
        await restartUnlessWorking(seenOn: false)
    }

    /// The service restarted to read a switch again, never while a task runs or a restart is under way. `seenOn`: a
    /// fresh reading saw the switch on (APP-662), shown as such while it restarts.
    private func restartUnlessWorking(seenOn: Bool) async {
        guard !restarting else { return }
        if let status = try? await rpc.call("chat.status", params: ["peek": true]),
           let running = status["running"] as? [[String: Any]], !running.isEmpty { return }
        moveGrant(.restarting(seenOn: seenOn))
        await restartService()
    }

    /// The agent app in Finder, to drag into a pane's list that doesn't show FERB.
    func showAgentApp() {
        if let target = permissions?.grantTarget, !target.isEmpty { system.reveal(target) }
    }

    func restartService() async {
        restarting = true
        message = "Restarting FERB's background service…"
        var ok = await service.restart()
        if let why = await serviceSpawnFailure(within: serviceCheckAfterStart) { ok = await repairService(why) }
        restarting = false
        message = ok ? nil : serviceError ?? "FERB's background service couldn't be restarted."
        if ok { askedRestart.removeAll() }
        moveGrant(.checking)
        await refreshPermissions()
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

    /// The daemon connects the browsers here once it has Full Disk Access (DMN-507); what kept it from that is shown.
    /// The page-level problem line goes once the daemon no longer reports it (APP-883).
    func refreshBrowsers() async {
        do {
            browsers = BrowserSnapshot(reply: try await rpc.call("setup.browsers", params: [:]))
            clearNotAnswering()
            daemonSilent = false
            let problem = browsers?.problem ?? ""
            if !problem.isEmpty { message = problem }
            else if let shown = shownBrowserProblem, message == shown { message = nil }
            shownBrowserProblem = problem.isEmpty ? nil : problem
        } catch {
            message = Self.notAnswering
            daemonSilent = true
        }
    }

    /// The browsers' problem the page last showed as its message.
    private var shownBrowserProblem: String?

    /// Add to <browser> on one profile (HAND-413): FERB's Web Store page opens in that profile (the owner adds it;
    /// FERB never does), then `watchForExtension` waits for the extension to be there and on. False when the page
    /// couldn't be opened (`message` says why).
    @discardableResult
    func addExtension(_ profile: BrowserProfileRow, watch: Bool = true) async -> Bool {
        do {
            _ = try await rpc.call("setup.browser_open", params: ["browser": profile.browser, "folder": profile.folder])
        } catch {
            message = error.localizedDescription
            return false
        }
        if watch { watchForExtension(profile) }
        return true
    }

    /// The profile FERB is waiting on after Add to (Settings → Browsers shows the wait, with Stop waiting).
    @Published private(set) var addWait: BrowserProfileRow?
    private var addWatch: Task<Void, Never>?

    /// Reads the browsers every `grantPoll` until FERB's extension is on (or paired) in that profile, then says it can
    /// be paired; the owner pairs it (rule 5). Ends on Stop waiting, or when another wait starts.
    func watchForExtension(_ profile: BrowserProfileRow) {
        addWatch?.cancel()
        addWait = profile
        message = nil
        addWatch = Task { [weak self] in
            while !Task.isCancelled {
                guard let poll = self?.grantPoll else { return }
                do { try await Task.sleep(for: poll) } catch { return }
                guard let self, !Task.isCancelled else { return }
                await self.refreshBrowsers()
                guard let (browser, row) = self.browserProfile(profile.id), row.state == .on || row.state == .paired
                else { continue }
                self.addWait = nil
                self.message = row.state == .paired ? nil
                    : "FERB is on in \(ProfileName(label: browser.label, name: row.name, email: row.email).text). Press Pair to pair it."
                return
            }
        }
    }

    func stopWatchingForExtension() {
        addWatch?.cancel()
        addWatch = nil
        addWait = nil
    }

    /// A profile in the last browsers snapshot, with its browser, by its id ("browser/folder").
    func browserProfile(_ id: String) -> (BrowserRow, BrowserProfileRow)? {
        for browser in browsers?.browsers ?? [] {
            if let row = browser.profiles.first(where: { $0.id == id }) { return (browser, row) }
        }
        return nil
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

    /// Pair all as it happens (HAND-412): each profile's state as its result lands. The tour shows it in the notch
    /// and Settings under Pair all.
    @Published private(set) var pairProgress: PairProgress?

    /// The tour and Settings both show this message; nil only when every owner profile is paired (HAND-411). Pair all
    /// is followed live (`setup.browsers_pairing`); `progress` gets every change. A daemon without the live call gets
    /// the one reply as before.
    func pairAll(progress onChange: ((PairProgress) -> Void)? = nil) async {
        guard !pairing else { return }
        pairing = true
        defer { pairing = false }
        var progress = PairProgress()
        var received = 0
        var note: String?
        do {
            for try await event in rpc.stream("setup.browsers_pairing", params: [:]) {
                received += 1
                progress.apply(event)
                pairProgress = progress
                onChange?(progress)
            }
            if let reply = progress.reply { note = Self.pairAllMessage(reply) }
            else if received == 0 { note = await pairAllOnce() }
            else { note = Self.pairAllEnded }
        } catch {
            if received == 0 { note = await pairAllOnce() } else { note = error.localizedDescription }
        }
        message = nil
        await refreshBrowsers()
        if let note { message = note }
    }

    /// Pair all as one reply, for a daemon without the live call (it refused it, or streamed nothing).
    private func pairAllOnce() async -> String? {
        do { return Self.pairAllMessage(try await rpc.call("setup.browsers_pair_all", params: [:], wait: 90)) }
        catch { return error.localizedDescription }
    }

    static let pairAllEnded = "Pair all ended before it said how it went. Try again."

    static let noBrowserProfiles = "FERB found no browser profiles on this Mac. Open your browser once, then try again."

    /// Pair all's reply in the owner's words, one line per profile it didn't pair: each skipped profile's step (not
    /// added yet, turned off, removed, too old) and each pairing that failed. nil only when every owner profile is
    /// paired; none found says so. A reply without `profiles` (an older daemon) names only the failed pairings.
    static func pairAllMessage(_ reply: [String: Any]) -> String? {
        func line(_ row: [String: Any], _ text: String?) -> String {
            let name = ProfileName(label: row["label"] as? String ?? "",
                                   name: row["name"] as? String ?? row["folder"] as? String ?? "a profile",
                                   email: row["email"] as? String ?? "")
            return name.text + ": " + (text ?? "it didn't pair")
        }
        let skipped = (reply["skipped"] as? [[String: Any]] ?? []).map { line($0, $0["step"] as? String) }
        let failed = (reply["results"] as? [[String: Any]] ?? []).filter { $0["paired"] as? Bool != true }
            .map { line($0, $0["why"] as? String) }
        if let total = reply["profiles"] as? Int, total == 0 { return noBrowserProfiles }
        let lines = skipped + failed
        return lines.isEmpty ? nil : lines.map { $0.hasSuffix(".") ? $0 : $0 + "." }.joined(separator: "\n")
    }

    // MARK: FERB's own account (APP-1)

    /// FERB's account as set now and, for the address typed (else the one set), the profiles signed into it.
    func refreshIdentity() async {
        let address = identityAddress.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        do {
            let state = IdentityState(try await rpc.call("identity.find", params: address.isEmpty ? [:] : ["address": address]))
            identity = state
            if identityAddress.isEmpty && !state.address.isEmpty { identityAddress = state.address }
            clearNotAnswering()
        } catch { identityLine = error.localizedDescription }
    }

    /// The owner's pick of the profile holding FERB's account: written by the daemon (it checks the profile holds only
    /// that account), then paired, which opens the extension's pairing page in that profile.
    func useIdentity(_ candidate: IdentityCandidate) async {
        guard !identityBusy else { return }
        identityBusy = true
        defer { identityBusy = false }
        let address = identityAddress.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        do {
            _ = try await rpc.call("identity.choose", params: ["address": address, "browser": candidate.browser,
                                                               "folder": candidate.folder])
            identityLine = "Pairing \(ProfileName(name: candidate.name, email: candidate.email).quoted): turn FERB's extension on there if the browser asks…"
            await pairIdentity(confirm: identity?.paired == true)
        } catch { identityLine = error.localizedDescription }
        await refreshIdentity()
    }

    func pairIdentity(confirm: Bool = false) async {
        do {
            let reply = try await rpc.call("identity.pair", params: ["confirm": confirm], wait: 90)
            identityLine = reply["paired"] as? Bool == true ? "FERB's own account is set up and paired."
                : "Not paired yet: \(reply["hint"] as? String ?? "no answer from FERB's extension in that profile")."
        } catch { identityLine = error.localizedDescription }
    }

    /// FERB stops using an account of its own (its profile's pairing stays in Browsers until unpaired there).
    func removeIdentity() async {
        do {
            _ = try await rpc.call("identity.choose", params: ["clear": true])
            identityAddress = ""
            identityLine = "FERB has no account of its own now."
        } catch { identityLine = error.localizedDescription }
        await refreshIdentity()
    }

    /// Skipped: the owner's profile FERB browses in instead (`browsing.owner_profile`, the settings path); "" for none.
    func browseAsOwner(in profile: String) async {
        do {
            _ = try await rpc.call("settings.set", params: ["key": "browsing.owner_profile", "value": profile])
            identityLine = profile.isEmpty ? nil : "FERB browses as you in that profile."
        } catch { identityLine = error.localizedDescription }
        await refreshIdentity()
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

    /// `setup.finish` with FERB.app's own rows: the daemon records setup as done only when every permission is on.
    func recordFinished() async {
        let own = Dictionary(uniqueKeysWithValues: appRows.map { ($0.id, $0.status.rawValue) })
        _ = try? await rpc.call("setup.finish", params: ["app": own])
    }
}
