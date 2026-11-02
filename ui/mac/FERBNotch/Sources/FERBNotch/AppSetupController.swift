import AppKit
import SwiftUI
import NotchCore

enum AppName {
    static var current: String { Bundle.main.object(forInfoDictionaryKey: "CFBundleName") as? String ?? "FERB" }
}

/// FERB.app's setup (L27, L34 C, kb area app). A first launch runs the first-run intro (IntroController): its
/// Meet sets the background service up from this app (`ferb setup --service`, no questions) and turns it on with macOS
/// (SMAppService: its definition is inside this signed app, DMN-2), and every later beat reads and
/// acts through the daemon. After the app is replaced by a new build, or when launchd doesn't have FERB running, launch
/// updates the service quietly (`ferb setup --service --update`) and runs only the intro's beats for a required grant
/// that's missing. A skipped intro (or a grant still off at its end) leaves the compact setup window on the first
/// required step left. Another installation (a checkout, or FERB.app elsewhere) is replaced only after the owner says
/// so, from the menu too. Replay intro (Settings → General, the menu's Set Up FERB…) runs every beat again. Closing
/// the window doesn't stop a service run; quitting FERB stops it, and setup finishes reinstalling the service first.
/// A development build, which has no Python inside, never installs anything.
@MainActor
final class AppSetupController: NSObject, NSWindowDelegate {
    private let bundle = Bundle.main.bundleURL.resolvingSymlinksInPath()
    private var window: NSWindow?
    var onFinished: (() -> Void)?
    /// Setup has nothing left to do (L34 Phase F): at launch when this build is set up, after a quiet update that
    /// needs no permission, or at the setup window's Done. The app applies the login-item default here (plan G adds
    /// "Updated to …").
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
    /// A repair ended the tour: its end leaves the compact window where the repair put it.
    private var repairing = false
    /// A tour is on, or ending.
    var touring: Bool { intro != nil }
    /// Where the intro stands (IntroProgress) and the setup window's Grant resume (SetupModel.resumeKey) live here.
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
    /// The user LaunchAgent FERB wrote before this app registered its own service (or a checkout's): setup retires it.
    private var agentPlist: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/LaunchAgents/com.ferb.daemon.plist")
    }
    /// What `ferb setup --service` last recorded: the build and app it set up (ferb/device/install.py SERVICE_RECORD).
    private var serviceRecord: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".ferb/service.json")
    }
    /// Made on first use: a setup with an injected model (tests, the intro playground) never touches SMAppService.
    lazy var background = AppServiceAgent()

    lazy var model = injectedModel ?? SetupModel(rpc: DaemonRPC(), runner: SetupCommand(launcher: launcher),
                                                 app: SystemAppPermissions(), system: SystemOpener(),
                                                 keys: KeychainAPIKeyStore(), service: DaemonService(),
                                                 background: isPackaged ? background : nil)

    var isPackaged: Bool {
        FileManager.default.isExecutableFile(atPath: launcher.path) && FileManager.default.fileExists(atPath: python.path)
            && FileManager.default.isExecutableFile(atPath: agentExecutable.path)
    }

    func startAtLaunch() {
        guard isPackaged else { return }
        let action = currentAction()
        Task { @MainActor in await self.decideAtLaunch(action) }
    }

    /// The launch decision for `action`. Only this build's running service is asked what's still missing: a new
    /// install has nothing to read yet, and an update reads after it has run (quietUpdate).
    func decideAtLaunch(_ action: AppSetupAction) async {
        let needs = action == .none ? await readNeeds(action) : Self.newInstall(action)
        let decision = AppSetupPlan.atLaunch(action, needs: needs, tour: progress.interrupted,
                                             resume: SetupModel.pendingResume(defaults))
        await launch(decision, action: action, needs: needs)
    }

    /// FERB isn't running and Restart couldn't start it: the service is set up again from the setup window, as
    /// Set Up FERB… did before the intro. A repair never starts a tour.
    func repairService() {
        guard isPackaged else { return }
        let action = currentAction()
        if case .confirmReplace(let current) = action, !confirmReplacing(current) { return }
        repair(action)
    }

    /// The repair wins over a tour: the tour is skipped (its own teardown: orb, sound, lines), then one compact
    /// window shows on Welcome, and the tour's end doesn't move it. A "Do it for real" the owner tapped waits in the
    /// composer: nothing is sent while the service is down.
    func repair(_ action: AppSetupAction) {
        if let intro {
            repairing = true
            intro.skip(forRepair: true)
        }
        show(.welcome, action: action)
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
    func launch(_ decision: SetupLaunch, action: AppSetupAction, needs: IntroNeeds) async {
        switch decision {
        case .nothing:
            if !needs.setupDone && progress.done && model.permissions != nil { await model.recordFinished() }
            onSettled?()
        case .quietUpdate: quietUpdate()
        case .window(let step): show(step, action: action)
        case .intro(let beats):
            if case .confirmReplace(let current) = action, !confirmReplacing(current) { return }
            startTour(beats, action: action, firstRun: needs.firstRun)
        }
    }

    /// One tour: its controller, orb and sound engine live until it ends. A tour can't start without the notch.
    private func startTour(_ beats: [IntroBeat], action: AppSetupAction, firstRun: Bool) {
        guard intro == nil else { return }
        guard let notch, let introControls else { show(.welcome, action: action); return }
        if !model.serviceRunning { model.serviceAction = action }
        window?.close()                                       // the compact window gives way to the tour
        defaults.removeObject(forKey: SetupModel.resumeKey)   // the intro's own resume wins
        var progress = self.progress
        if firstRun { progress.resumeBeat = nil }            // a new install starts at Meet, whatever a quit left
        progress.plan = beats
        let (orb, sound) = makeTourParts()
        let controller = IntroController(setup: model, orb: orb, sound: sound, notch: notch, progress: progress,
                                         controls: introControls, sleeper: introSleeper)
        controller.onFinished = { [weak self, weak controller] in
            self?.tourEnded(left: controller?.remainingRequired ?? [])
        }
        self.sound = sound
        self.orb = orb
        intro = controller
        controller.start(beats: beats, firstRun: firstRun)
        if !controller.running { tourEnded(left: []) }       // nothing in the plan to run
    }

    /// The tour ended (finished or skipped): the compact window on the first required step left, else settled.
    private func tourEnded(left: [String]) {
        intro = nil
        orb = nil
        sound = nil
        var progress = self.progress
        progress.plan = nil
        defaults.removeObject(forKey: SetupModel.resumeKey)   // the tour's grants left it
        if repairing {
            repairing = false
            return
        }
        let serviceSetUp = model.serviceAction == .none && !model.serviceRunning
        if let step = AppSetupPlan.afterIntro(left, serviceSetUp: serviceSetUp) {
            show(step, action: model.serviceAction)
        } else {
            onSettled?()
        }
    }

    /// What this install still needs, read from the daemon and the system: only what the decision depends on. A
    /// quit tour or a first run needs nothing more; a finished setup needs nothing more at launch and only its grants
    /// after an update. The launch's reads leave no "isn't answering" line behind on the shared model (the daemon
    /// may only be slow to start; the setup window's own polling says so if it lasts).
    private func readNeeds(_ action: AppSetupAction) async -> IntroNeeds {
        let done = await model.setupDone()
        let progress = self.progress
        let firstRun = AppSetupPlan.firstRun(action, setupDone: done, introDone: progress.done,
                                             resuming: progress.resumeBeat != nil)
        if progress.interrupted == nil && !firstRun {
            if !done || action == .update { await model.refreshPermissions() }
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
        let legacy = (NSDictionary(contentsOf: agentPlist) as? [String: Any]).flatMap {
            $0["Label"] as? String == LaunchctlPlan.label ? $0 : nil
        }
        let record = (try? Data(contentsOf: serviceRecord)).flatMap {
            try? JSONSerialization.jsonObject(with: $0) as? [String: Any]
        }
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? ""
        return AppSetupPlan.decide(legacyAgent: legacy, service: background.status, record: record,
                                   agentExecutable: agentExecutable.path, appPath: bundle.path, build: build,
                                   daemonRunning: daemonRunning())
    }

    /// Uninstall: the background service unregistered from macOS (the definition goes with the app itself), before
    /// `ferb uninstall` runs. Nil when it's off, else why not.
    func unregisterService() async -> String? {
        guard isPackaged else { return nil }
        return await background.unregisterForUninstall()
    }

    private func quietUpdate() {
        model.serviceAction = .update
        model.runService { [weak self] ok in
            guard let self else { return }
            guard ok else { self.show(.welcome, action: .update); return }
            Task { @MainActor in
                let needs = await self.readNeeds(.update)
                let decision = AppSetupPlan.afterUpdate(needs, tour: self.progress.interrupted)
                await self.launch(decision, action: .none, needs: needs)
            }
        }
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

    private func show(_ step: SetupStep, action: AppSetupAction) {
        if !model.serviceRunning { model.serviceAction = action }
        if window == nil {
            let created = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 560, height: 550),
                                   styleMask: [.titled, .closable, .miniaturizable, .fullSizeContentView], backing: .buffered, defer: false)
            created.title = "Set Up \(AppName.current)"
            created.isReleasedWhenClosed = false
            created.delegate = self
            created.titleVisibility = .hidden
            created.titlebarAppearsTransparent = true
            created.isOpaque = false
            created.backgroundColor = .clear
            created.isMovableByWindowBackground = true
            for button in [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton] {
                created.standardWindowButton(button)?.isHidden = true
            }
            created.contentView = NSHostingView(rootView: SetupView(model: model, close: { [weak created] in
                created?.performClose(nil)
            }))
            created.contentView?.wantsLayer = true
            created.contentView?.layer?.cornerRadius = 28
            created.contentView?.layer?.masksToBounds = true
            created.center()
            window = created
            model.onFinish = { [weak self] in
                self?.window?.close()
                self?.onFinished?()
                self?.onSettled?()
            }
        }
        model.go(step)
        model.startPolling()
        NSApp.activate(ignoringOtherApps: true)          // the owner opened FERB or chose Set Up FERB…
        window?.makeKeyAndOrderFront(nil)
    }

    /// The compact setup window, when it exists (tests).
    var setupWindow: NSWindow? { window }

    func windowWillClose(_ notification: Notification) {
        model.stopPolling()           // a service run keeps going: stopping it halfway could leave the service unloaded
    }
}
