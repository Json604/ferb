import AppKit
import SwiftUI
import NotchCore

enum AppName {
    static var current: String { Bundle.main.object(forInfoDictionaryKey: "CFBundleName") as? String ?? "FERB" }
}

/// FERB.app's setup (L27, L34 C, notes/setup-intro.md). A first launch runs the first-run intro (IntroController): its
/// Meet sets the background service up from this app (`ferb setup --service`, no questions) and turns it on with macOS
/// (SMAppService: its definition is inside this signed app, notes/daemon.md decision 2), and every later beat reads and
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
    private var orb: OrbController?
    private var sound: FerbSoundEngine?
    private var progress: IntroProgress { IntroProgress(defaults: .standard) }

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
    let background = AppServiceAgent()

    lazy var model = SetupModel(rpc: DaemonRPC(), runner: SetupCommand(launcher: launcher),
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
        Task { @MainActor in
            // Only this build's running service is asked what's still missing: a new install has nothing to read
            // yet, and an update reads after it has run (quietUpdate).
            let needs = action == .none ? await self.readNeeds(action) : Self.newInstall(action)
            let decision = AppSetupPlan.atLaunch(action, needs: needs, tour: self.progress.interrupted,
                                                 resume: SetupModel.pendingResume(.standard))
            self.launch(decision, action: action, firstRun: needs.firstRun)
        }
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

    private func launch(_ decision: SetupLaunch, action: AppSetupAction, firstRun: Bool) {
        switch decision {
        case .nothing: onSettled?()
        case .quietUpdate: quietUpdate()
        case .window(let step): show(step, action: action)
        case .intro(let beats):
            if case .confirmReplace(let current) = action, !confirmReplacing(current) { return }
            startTour(beats, action: action, firstRun: firstRun)
        }
    }

    /// One tour: its controller, orb and sound engine live until it ends. A tour can't start without the notch.
    private func startTour(_ beats: [IntroBeat], action: AppSetupAction, firstRun: Bool) {
        guard intro == nil else { return }
        guard let notch, let introControls else { show(.welcome, action: action); return }
        if !model.serviceRunning { model.serviceAction = action }
        window?.close()                                       // the compact window gives way to the tour
        UserDefaults.standard.removeObject(forKey: SetupModel.resumeKey)   // the intro's own resume wins
        var progress = self.progress
        if firstRun { progress.resumeBeat = nil }            // a new install starts at Meet, whatever a quit left
        progress.plan = beats
        let sound = FerbSoundEngine()
        let orb = OrbController(sound: sound)
        let controller = IntroController(setup: model, orb: orb, sound: sound, notch: notch, progress: progress,
                                         controls: introControls)
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
        UserDefaults.standard.removeObject(forKey: SetupModel.resumeKey)   // the tour's grants left it
        let serviceSetUp = model.serviceAction == .none && !model.serviceRunning
        if let step = AppSetupPlan.afterIntro(left, serviceSetUp: serviceSetUp) {
            show(step, action: model.serviceAction)
        } else {
            onSettled?()
        }
    }

    /// What this install still needs, read from the daemon and the system.
    private func readNeeds(_ action: AppSetupAction) async -> IntroNeeds {
        let done = await model.setupDone()
        let progress = self.progress
        let firstRun = AppSetupPlan.firstRun(action, setupDone: done, introDone: progress.done,
                                             resuming: progress.resumeBeat != nil)
        await model.refreshAI()
        await model.refreshPermissions()
        await model.refreshBrowsers()
        return snapshotNeeds(setupDone: done, firstRun: firstRun)
    }

    /// IntroNeeds from the setup model's last reads. A fact the daemon didn't answer asks for nothing: no grant
    /// counts as missing and browsers as unpaired only when some were found and none is paired.
    private func snapshotNeeds(setupDone: Bool, firstRun: Bool) -> IntroNeeds {
        let missing = model.permissions == nil ? [] : SetupModel.allowAllOrder(model.setupRows)
        let profiles = model.browsers?.browsers.flatMap(\.profiles) ?? []
        let unpaired = !profiles.isEmpty && !profiles.contains { $0.state == .paired }
        return IntroNeeds(setupDone: setupDone, signedIn: IntroController.connected(model.ai),
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
                self.launch(decision, action: .none, firstRun: needs.firstRun)
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
            let created = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 560, height: 500),
                                   styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false)
            created.title = "Set Up \(AppName.current)"
            created.isReleasedWhenClosed = false
            created.delegate = self
            created.contentView = NSHostingView(rootView: SetupView(model: model))
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

    func windowWillClose(_ notification: Notification) {
        model.stopPolling()           // a service run keeps going: stopping it halfway could leave the service unloaded
    }
}
