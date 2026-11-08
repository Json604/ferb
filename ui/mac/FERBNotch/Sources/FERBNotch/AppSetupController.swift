import AppKit
import Combine
import SwiftUI
import NotchCore

enum AppName {
    static var current: String { Bundle.main.object(forInfoDictionaryKey: "CFBundleName") as? String ?? "FERB" }
}

/// FERB.app's setup (L27, L34 C, kb area app). A first launch runs the first-run intro (IntroController): its
/// Meet sets the background service up from this app (`ferb setup --service`, no questions: it writes and loads the
/// user LaunchAgent, DMN-513), and every later beat reads and acts through the daemon. After the app is replaced by a
/// new build, an older install's LaunchAgent or a 2026-11 build's SMAppService registration is found, the LaunchAgent
/// differs from what setup wrote (DMN-514) or launchd doesn't have FERB running, launch updates the service quietly
/// (`ferb setup --service --update`) and runs only the intro's beats for a required grant that's missing. Setup and its
/// errors live in the notch tour only (APP-685, owner 2026-11-04): a failed service install or update is the tour's
/// service line with its reason and Try again, and a launch with setup unfinished resumes the tour at its saved beat;
/// there is no setup window. A skipped intro (or a grant still off at its end) comes back as the tour at the next
/// launch. A checkout's LaunchAgent is replaced only after the owner says so, from the menu too. Replay intro
/// (Settings → General, the menu's Set Up FERB…) runs every beat again. Quitting FERB stops a service run, and setup
/// finishes reinstalling the service first. A development build, which has no Python inside, never installs anything.
@MainActor
final class AppSetupController: NSObject {
    private let bundle = Bundle.main.bundleURL.resolvingSymlinksInPath()
    /// Setup has nothing left to do (L34 Phase F): at launch when this build is set up, after a quiet update that
    /// needs no permission, or at the tour's end with nothing required left. The app applies the login-item default
    /// here (plan G adds "Updated to …").
    var onSettled: (() -> Void)?
    /// The notch the intro speaks in and the controls it teaches (PanelCoordinator's), set before launch.
    var notch: NotchModel?
    var introControls: IntroControlFeeding?
    /// The running intro, with the orb and the sound engine it plays: made when a tour starts, released when it ends.
    private var intro: IntroController?
    private var orb: OrbControlling?
    private var sound: FerbSoundPlaying?
    /// The only place the real orb is made, with the app's one sound engine (AppDelegate passes it; tests make fakes).
    var makeTourParts: @MainActor () -> (orb: OrbControlling, sound: FerbSoundPlaying) = {
        let sound = FerbSoundEngine()
        return (OrbController(sound: sound), sound)
    }
    /// The tour's waits (nil: real time). Only the intro playground sets it, to run the tour faster or slower.
    var introSleeper: IntroSleeping?
    /// A tour is on, or ending (the service step of a repair included).
    var touring: Bool { intro != nil }
    /// Where the intro stands (IntroProgress) lives here.
    private let defaults: UserDefaults
    private var progress: IntroProgress { IntroProgress(defaults: defaults) }
    private let injectedModel: SetupModel?

    /// `model` and `defaults` are for tests: the app uses its own setup model and the standard defaults.
    init(model: SetupModel? = nil, defaults: UserDefaults = .standard) {
        injectedModel = model
        self.defaults = defaults
        super.init()
    }

    private var python: URL { bundle.appendingPathComponent("Contents/Resources/python/bin/python3.12") }
    private var agentExecutable: URL { bundle.appendingPathComponent("Contents/Helpers/FERB.app/Contents/MacOS/FERB") }
    private var launcher: URL { bundle.appendingPathComponent("Contents/Resources/bin/ferb") }
    /// The background service's user LaunchAgent (this app's, an older install's or a checkout's; DMN-513).
    private var agentPlist: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/LaunchAgents/com.ferb.daemon.plist")
    }
    /// What `ferb setup --service` last recorded: the build and app it set up (ferb/device/install.py SERVICE_RECORD).
    private var serviceRecord: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".ferb/service.json")
    }
    /// Made on first use: a setup with an injected model (tests, the intro playground) never touches SMAppService or
    /// launchd.
    lazy var background = LaunchAgentService()
    /// The first-launch entrance a first run's tour draws over the clip (APP-618); the playground picks its own.
    var makeArrival: () -> IntroArrivalShowing? = { StartupArrival() }

    lazy var model: SetupModel = {
        if let injectedModel { return injectedModel }
        let rpc = DaemonRPC()
        return SetupModel(rpc: rpc, runner: SetupCommand(launcher: launcher), app: SystemAppPermissions(),
                          system: SystemOpener(), keys: DaemonAPIKeyStore(rpc: rpc), service: DaemonService(),
                          background: isPackaged ? background : nil)
    }()

    var isPackaged: Bool {
        FileManager.default.isExecutableFile(atPath: launcher.path) && FileManager.default.fileExists(atPath: python.path)
            && FileManager.default.isExecutableFile(atPath: agentExecutable.path)
    }

    func startAtLaunch() {
        guard isPackaged else { return }
        let action = currentAction()
        watchServiceSetup(pending: action != .none)
        Task { @MainActor in await self.decideAtLaunch(action) }
    }

    private var serviceSetupWatch: AnyCancellable?

    /// The notch's menu bar follows whether setup is setting the background service up (APP-698): from launch, when
    /// the service still has to be installed or updated (`pending`), until it runs, and again during any restart.
    func watchServiceSetup(pending: Bool) {
        notch?.serviceSettingUp = pending || model.settingUpService
        serviceSetupWatch = model.objectWillChange.sink { [weak self] _ in
            DispatchQueue.main.async {                 // objectWillChange fires before the change lands
                guard let self else { return }
                self.notch?.serviceSettingUp = self.model.settingUpService
            }
        }
    }

    /// The launch decision for `action`. Only this build's running service is asked what's still missing: a new
    /// install has nothing to read yet, and an update reads after it has run (quietUpdate).
    func decideAtLaunch(_ action: AppSetupAction) async {
        let needs = action == .none ? await readNeeds(action) : Self.newInstall(action)
        let decision = AppSetupPlan.atLaunch(action, needs: needs, tour: progress.interrupted)
        await launch(decision, action: action, needs: needs)
    }

    /// FERB isn't running and Restart couldn't start it: the service is set up again in the notch (APP-685).
    func repairService() {
        guard isPackaged else { return }
        let action = currentAction()
        if case .confirmReplace(let current) = action, !confirmReplacing(current) { return }
        repair(action)
    }

    /// The repair wins over a tour: the tour is interrupted where it is (its own teardown: orb, sound, lines; its
    /// place kept), the service step runs alone in the notch, its failure shown with the reason and Try again, and
    /// once the service runs the launch goes on as after a quiet update, resuming that tour (APP-685). A task the
    /// tour started keeps its own turn.
    func repair(_ action: AppSetupAction) {
        if let intro, !intro.repairing {
            intro.interrupt()
            self.intro = nil
            orb = nil
            sound = nil
        }
        guard intro == nil else { return }               // a repair already shows
        if !model.serviceRunning { model.serviceAction = action == .none ? .update : action }
        startRepair(update: false)
    }

    /// The service step alone in the notch; once the service runs, the launch's decision (the tour, or nothing).
    private func startRepair(update: Bool) {
        guard let notch, let introControls else { return }
        let (orb, sound) = makeTourParts()
        let controller = IntroController(setup: model, orb: orb, sound: sound, notch: notch, progress: progress,
                                         controls: introControls, sleeper: introSleeper)
        controller.onFinished = { [weak self, weak controller] in
            guard let self else { return }
            let repaired = controller?.repaired ?? false
            self.intro = nil
            self.orb = nil
            self.sound = nil
            if repaired { Task { @MainActor in await self.afterQuietUpdate(update: update) } }
        }
        self.orb = orb
        self.sound = sound
        intro = controller
        controller.startRepair()
    }

    /// Set Up FERB… and Settings → General → Replay intro: every beat again, ignored while a tour runs. It resets no
    /// setup, sign-in or grant; Meet reinstalls only when this app's build isn't the one running.
    func replayIntro() {
        guard isPackaged, intro == nil else { return }
        let action = currentAction()
        if case .confirmReplace(let current) = action, !confirmReplacing(current) { return }
        var progress = self.progress
        progress.resumeBeat = nil                     // a replay starts at Meet, not where a quit tour stopped
        guard case .intro(let beats) = AppSetupPlan.replay(snapshotNeeds(setupDone: true, firstRun: false)) else { return }
        startTour(beats, action: action, firstRun: false)
    }

    /// The one path every launch decision runs through. Nothing to do, with the tour behind it and every grant read,
    /// but setup never recorded (a skip or Home whose `setup.finish` didn't land): it is recorded quietly now; the
    /// daemon checks every row again before it counts setup as done.
    /// `startup`: a newer build resumes a tour a quit interrupted, opening with the startup again (APP-645).
    func launch(_ decision: SetupLaunch, action: AppSetupAction, needs: IntroNeeds, startup: Bool = false) async {
        switch decision {
        case .nothing:
            if !needs.setupDone && progress.done && model.permissions != nil { await model.recordFinished() }
            onSettled?()
        case .quietUpdate: quietUpdate()
        case .intro(let beats):
            if case .confirmReplace(let current) = action, !confirmReplacing(current) { return }
            startTour(beats, action: action, firstRun: needs.firstRun, startup: startup)
        }
    }

    /// One tour: its controller, orb and sound engine live until it ends. A tour can't start without the notch.
    private func startTour(_ beats: [IntroBeat], action: AppSetupAction, firstRun: Bool, startup: Bool = false) {
        guard intro == nil else { return }
        guard let notch, let introControls else { return }
        if !model.serviceRunning { model.serviceAction = action }
        var progress = self.progress
        if firstRun { progress.resumeBeat = nil }            // a new install starts at Meet, whatever a quit left
        progress.plan = beats
        if beats == IntroBeat.allCases { progress.tourSeen = true }    // this install has had the whole tour (APP-635)
        let (orb, sound) = makeTourParts()
        let controller = IntroController(setup: model, orb: orb, sound: sound, notch: notch, progress: progress,
                                         controls: introControls, sleeper: introSleeper,
                                         arrival: firstRun || startup ? makeArrival() : nil)
        controller.onFinished = { [weak self, weak controller] in
            self?.tourEnded(left: controller?.remainingRequired ?? [])
        }
        self.sound = sound
        self.orb = orb
        intro = controller
        controller.start(beats: beats, firstRun: firstRun, startup: startup)
        if !controller.running { tourEnded(left: []) }       // nothing in the plan to run
    }

    /// The tour ended (finished or skipped): settled when nothing required is left; otherwise nothing opens, and the
    /// next launch brings the tour back for what is missing (APP-508, APP-685: never a window).
    private func tourEnded(left: [String]) {
        intro = nil
        orb = nil
        sound = nil
        var progress = self.progress
        progress.plan = nil
        let serviceSetUp = model.serviceAction == .none && !model.serviceRunning
        if left.isEmpty && serviceSetUp { onSettled?() }
    }

    /// What this install still needs, read from the daemon and the system: only what the decision depends on. A
    /// quit tour or a first run needs nothing more; a finished setup needs only its grants, read at every launch so a
    /// grant switched off since is noticed (APP-686). The launch's reads leave no "isn't answering" line behind on the
    /// shared model (the daemon may only be slow to start).
    private func readNeeds(_ action: AppSetupAction) async -> IntroNeeds {
        let done = await model.setupDone()
        let progress = self.progress
        let firstRun = AppSetupPlan.firstRun(action, setupDone: done, introDone: progress.done,
                                             resuming: progress.resumeBeat != nil, tourSeen: progress.tourSeen)
        if progress.interrupted == nil && !firstRun {
            await model.refreshPermissions()
            if !done {
                await model.refreshAI()
                await model.refreshBrowsers()
            }
            model.clearNotAnswering()
        }
        return snapshotNeeds(setupDone: done, firstRun: firstRun)
    }

    /// IntroNeeds from the setup model's last reads. A fact the daemon didn't answer (or that wasn't read) asks for
    /// nothing: signed in, no grant missing, browsers unpaired only when some were found and none is paired. A grant
    /// counts as missing as in Allow all: every setup row not on, FERB.app's own included, a blocked one never.
    func snapshotNeeds(setupDone: Bool, firstRun: Bool) -> IntroNeeds {
        let missing = model.permissions == nil ? [] : SetupModel.allowAllOrder(model.setupRows)
        let profiles = model.browsers?.browsers.flatMap(\.profiles) ?? []
        let unpaired = !profiles.isEmpty && !profiles.contains { $0.state == .paired }
        return IntroNeeds(setupDone: setupDone, signedIn: model.ai == nil || IntroController.connected(model.ai),
                          missingPermissions: missing, unpairedBrowsers: unpaired, firstRun: firstRun)
    }

    /// A new install's needs (nothing set up yet): the whole tour.
    private static func newInstall(_ action: AppSetupAction) -> IntroNeeds {
        IntroNeeds(setupDone: false, signedIn: false, missingPermissions: SetupModel.allowAllSequence,
                   unpairedBrowsers: true,
                   firstRun: AppSetupPlan.firstRun(action, setupDone: false, introDone: false, resuming: false))
    }

    /// FERB is quitting: stop a running setup command; it finishes reinstalling the background service first.
    func stop() { model.runner.cancel() }

    private func currentAction() -> AppSetupAction {
        let data = try? Data(contentsOf: agentPlist)
        let agent = data.flatMap {
            try? PropertyListSerialization.propertyList(from: $0, format: nil) as? [String: Any]
        }.flatMap { $0["Label"] as? String == LaunchctlPlan.label ? $0 : nil }
        let record = (try? Data(contentsOf: serviceRecord)).flatMap {
            try? JSONSerialization.jsonObject(with: $0) as? [String: Any]
        }
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? ""
        return AppSetupPlan.decide(launchAgent: agent, launchAgentDigest: agent == nil ? nil : data.map(AppSetupPlan.digest),
                                   registration: background.registration, record: record,
                                   agentExecutable: agentExecutable.path, appPath: bundle.path, build: build,
                                   daemonRunning: daemonRunning())
    }

    /// Uninstall: a 2026-11 build's SMAppService registration unregistered from macOS, before `ferb uninstall` boots
    /// the LaunchAgent out and deletes it. Nil when macOS lists none, else why not.
    func unregisterService() async -> String? {
        guard isPackaged else { return nil }
        return await background.unregisterForUninstall()
    }

    /// The service updated with nothing shown; if that fails, the notch shows the service step with the reason and
    /// Try again (APP-685, live 2026-11-04: the old compact window came up with "restarting the daemon failed:"), and
    /// once the service runs the launch goes on to its tour, resumed at its saved beat.
    private func quietUpdate() {
        model.serviceAction = .update
        model.runService { [weak self] ok in
            guard let self else { return }
            guard ok else { self.startRepair(update: true); return }
            Task { @MainActor in await self.afterQuietUpdate(update: true) }
        }
    }

    /// What the launch still asks for once the updated service runs.
    private func afterQuietUpdate(update: Bool) async {
        let needs = await readNeeds(.update)
        let interrupted = progress.interrupted
        let decision = AppSetupPlan.afterUpdate(needs, tour: interrupted)
        // A newer build over a tour a quit interrupted: the startup plays again before the tour resumes (APP-645).
        await launch(decision, action: .none, needs: needs, startup: update && interrupted != nil && !needs.firstRun)
    }

    /// Whether launchd has FERB's agent running now (`launchctl print` only reads launchd's state).
    private func daemonRunning() -> Bool {
        let launchctl = Process()
        launchctl.executableURL = URL(fileURLWithPath: "/bin/launchctl")
        launchctl.arguments = ["print", "gui/\(getuid())/com.ferb.daemon"]
        let pipe = Pipe()
        launchctl.standardOutput = pipe
        launchctl.standardError = FileHandle.nullDevice
        do { try launchctl.run() } catch { return false }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        launchctl.waitUntilExit()
        return launchctl.terminationStatus == 0 && String(decoding: data, as: UTF8.self).contains("state = running")
    }

    private func confirmReplacing(_ current: String) -> Bool {
        let alert = NSAlert()
        alert.messageText = "Set up FERB from this app?"
        alert.informativeText = "FERB's background service currently runs \(current). Setting up from this app " +
            "runs it from \(bundle.path) instead. Your data in ~/.ferb stays."
        alert.addButton(withTitle: "Set Up From This App")
        alert.addButton(withTitle: "Not Now")
        NSApp.activate(ignoringOtherApps: true)
        return alert.runModal() == .alertFirstButtonReturn
    }
}
