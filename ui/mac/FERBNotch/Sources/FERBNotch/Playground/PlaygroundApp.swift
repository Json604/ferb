#if FERB_PLAYGROUND
import AppKit
import SwiftUI
import NotchCore

/// The playground's app: FERB.app's own notch (PanelCoordinator), setup controller and intro, wired as AppDelegate
/// wires them, with every service behind them faked (PlaygroundServices) and none of FERB.app's background parts
/// (Sparkle, the login item, the background service, notifications, the FERB window). Plus the control panel.
@MainActor
final class PlaygroundDelegate: NSObject, NSApplicationDelegate {
    private let home: URL
    private let fresh: Bool
    private var services: PlaygroundServices?
    private var coordinator: PanelCoordinator?
    private var appSetup: AppSetupController?
    private var controls: NSWindow?
    private let arrival = PlaygroundArrival()
    private var appReview: PlaygroundAppReview?
    private let dock = DockPresence()

    init(home: URL, fresh: Bool) {
        self.home = home
        self.fresh = fresh
        super.init()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        installMenu()
        dock.start()
        let defaults = UserDefaults.standard              // the playground's own domain (dev.ferb.playground)
        if fresh { Self.clear(defaults) }
        let services = PlaygroundServices.make(home: home, defaults: defaults, fresh: fresh)
        self.services = services
        let state = services.state
        if !fresh { state.applyRestart() }                // a reopened FERB reads the switches flipped before the quit
        state.onSpeedChange = { services.applySpeed() }
        state.note(fresh ? "fresh start (home: \(home.path))" : "reopened (state kept)")

        let coordinator = PanelCoordinator(model: services.notch)
        coordinator.service = services.service            // the menu's Restart: faked, never launchctl
        let sound = PlaygroundSound(state: state, defaults: defaults)
        if PlaygroundMode.usesArrival {
            sound.onPlay = { [weak self] cue in
                if cue == .firstLaunch {
                    self?.arrival.show(seconds: (IntroController.firstLaunchClipSeconds() ?? 2) / state.speed)
                } else if cue == .intro { self?.arrival.hide() }
            }
            sound.onStop = { [weak self] in self?.arrival.hide() }
        }
        coordinator.model.feedback.player = sound
        self.coordinator = coordinator
        services.opener.avoidFrame = { [weak coordinator] in coordinator?.introScreenFrame }
        services.opener.onInteraction = { [weak coordinator] window in coordinator?.introCompanionFocused(window) }

        let appSetup = AppSetupController(model: services.setup, defaults: defaults)
        appSetup.introSleeper = services.sleeper
        appSetup.makeTourParts = { [opener = services.opener] in
            (OrbController(sound: sound, clock: PlaygroundClock(clock: DisplayLinkClock(), state: state),
                           makeTracker: { PlaygroundSettingsTracker(opener: opener) }), sound)
        }
        appSetup.notch = services.notch
        appSetup.introControls = coordinator.introControls
        appSetup.onFinished = { [weak coordinator] in coordinator?.showFromMenu() }
        appSetup.onSettled = { state.note("setup settled: nothing left for the intro") }
        self.appSetup = appSetup
        services.opener.onQuitAndReopen = { [weak self] in self?.relaunch(reopen: true) }

        let review = PlaygroundAppReview(services: services, coordinator: coordinator,
            openReview: { [weak self] in self?.relaunch(reopen: false, appReview: true) },
            replayIntro: { [weak self] in self?.relaunch(reopen: false) })
        appReview = review
        if PlaygroundMode.startsInAppReview {
            do { try review.prepare() } catch { state.note("Could not prepare the app review: " + error.localizedDescription) }
        }
        coordinator.start()
        showControls(services: services, coordinator: coordinator)
        // A fresh start is a new install (the whole tour, with the first-launch clip); a reopen is a set-up app whose
        // tour a quit interrupted (it resumes at its beat), exactly as FERB.app decides at launch.
        if PlaygroundMode.startsInAppReview {
            Task {
                await services.setup.refreshAI()
                await services.setup.refreshPermissions()
                await services.notch.refreshDaemonStatus()
                review.play(.conversation)
                review.show(.schedules)
            }
        } else {
            Task { await appSetup.decideAtLaunch(fresh ? .install : .none) }
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        controls?.makeKeyAndOrderFront(nil)
        return true
    }

    func applicationWillTerminate(_ notification: Notification) {
        arrival.hide()
        appSetup?.stop()
        coordinator?.tearDown()
    }

    // MARK: Relaunching

    /// Quits and opens the playground again: `reopen` keeps the fake world and the intro's progress (macOS's "Quit &
    /// Reopen"); otherwise everything starts fresh. The new copy starts once this one has exited.
    func relaunch(reopen: Bool, comparing: Bool = false, appReview: Bool = false) {
        services?.state.note(reopen ? "quitting and reopening…" : "restarting fresh…")
        let script = "while /bin/kill -0 \(getpid()) 2>/dev/null; do /bin/sleep 0.1; done; " +
            "/usr/bin/open -n --env \"FERB_HOME=$1\" \"$0\" --args \"${@:2}\""
        let shell = Process()
        shell.executableURL = URL(fileURLWithPath: "/bin/bash")
        let otherName = PlaygroundMode.usesArrival ? "FERB Setup Concept.app" : "FERB Arrival Concept.app"
        let otherHome = PlaygroundMode.usesArrival ? "ferb-setup-concept" : "ferb-arrival-concept"
        let appPath = comparing ? Bundle.main.bundleURL.deletingLastPathComponent().appendingPathComponent(otherName).path
                                : Bundle.main.bundlePath
        let dataPath = comparing ? home.deletingLastPathComponent().appendingPathComponent(otherHome).path : home.path
        guard FileManager.default.fileExists(atPath: appPath) else {
            services?.state.note("Build the other concept first: \(otherName)")
            return
        }
        shell.arguments = ["-c", script, appPath, dataPath] + (reopen ? [PlaygroundMode.reopenArgument] : [])
            + (appReview ? [PlaygroundMode.appReviewArgument] : [])
        do { try shell.run() } catch {
            services?.state.note("couldn't relaunch: \(error.localizedDescription)")
            return
        }
        NSApp.terminate(nil)
    }

    /// The tour resumes at `beat` after a reopen; `completeEarlier` marks every step before it done first.
    func jump(to beat: IntroBeat, completeEarlier: Bool) {
        guard let services else { return }
        if completeEarlier { services.state.completeEarlierSteps(before: beat) }
        var progress = IntroProgress(defaults: .standard)
        progress.done = false
        progress.plan = IntroBeat.allCases
        progress.resumeBeat = beat
        relaunch(reopen: true)
    }

    /// Where the notch sits: in the camera housing, or on a screen edge (the product's settings keys), then laid out
    /// again as after a display change.
    func place(_ placement: PlaygroundPlacement, display: CGDirectDisplayID?) {
        let defaults = UserDefaults.standard
        switch placement {
        case .notch:
            NotchDocking.save(true, in: defaults)
        case .edge(let edge):
            NotchDocking.save(false, in: defaults)
            defaults.set(edge.rawValue, forKey: "notch.indicatorEdge")
            defaults.set(0.5, forKey: "notch.indicatorFraction")
        }
        if let display { defaults.set(NSNumber(value: display), forKey: "notch.indicatorDisplayID") }
        coordinator?.displayParametersChanged()
        services?.state.note("notch placed: \(placement.label)")
    }

    /// A fresh start forgets everything the last run kept, except where the playground's own windows were and where
    /// the notch was placed (the panel's Notch setting, the product's own `notch.*` keys).
    private static func clear(_ defaults: UserDefaults) {
        let keep = (defaults.persistentDomain(forName: PlaygroundMode.bundleID) ?? [:])
            .filter { $0.key.hasPrefix("NSWindow Frame Playground") || $0.key.hasPrefix("notch.") || $0.key.hasPrefix("concept.") }
        defaults.setPersistentDomain(keep, forName: PlaygroundMode.bundleID)
    }

    private func installMenu() {
        let menu = NSMenu()
        let appItem = NSMenuItem()
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "Quit FERB Playground", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appItem.submenu = appMenu
        menu.addItem(appItem)
        let editItem = NSMenuItem()
        let edit = NSMenu(title: "Edit")
        for (title, selector, key) in [("Undo", "undo:", "z"), ("Redo", "redo:", "Z"), ("Cut", "cut:", "x"),
                                        ("Copy", "copy:", "c"), ("Paste", "paste:", "v"),
                                        ("Select All", "selectAll:", "a")] {
            edit.addItem(withTitle: title, action: Selector(selector), keyEquivalent: key)
        }
        editItem.submenu = edit
        menu.addItem(editItem)
        NSApp.mainMenu = menu
    }

    private func showControls(services: PlaygroundServices, coordinator: PanelCoordinator) {
        let actions = PlaygroundActions(
            restartFresh: { [weak self] in self?.relaunch(reopen: false) },
            quitAndReopen: { [weak self] in self?.relaunch(reopen: true, appReview: services.state.exploringApp) },
            jump: { [weak self] beat, complete in self?.jump(to: beat, completeEarlier: complete) },
            restartService: { Task { _ = await services.service.restart() } },
            control: { [weak coordinator] control in coordinator?.introControls.saw(control) },
            stopPreview: { services.notch.introAction(.stopPreview) },
            openNotch: { [weak coordinator] in coordinator?.showFromMenu() },
            closeNotch: { services.notch.collapse() },
            place: { [weak self] placement, display in self?.place(placement, display: display) },
            line: { services.notch.introLine.map { [$0.text, $0.detail ?? ""].filter { !$0.isEmpty }.joined(separator: " — ") } },
            compareStartup: { [weak self] in self?.relaunch(reopen: false, comparing: true) },
            exploreApp: { [weak self] in self?.relaunch(reopen: false, appReview: true) },
            showSection: { [weak self] section in self?.appReview?.show(section) },
            showScene: { [weak self] scene in self?.appReview?.play(scene) },
            finishTask: { services.daemon.workspace.finishRunning() },
            showTaskPreview: { [weak self] in self?.appReview?.showPreview() },
            previewSetup: { [weak self] in
                self?.arrival.hide()
                self?.appSetup?.repair(.none)
                services.setup.go(.ai)
            })
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 440, height: 820),
                              styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window.title = services.state.exploringApp ? "FERB app playground — controls" : PlaygroundMode.usesArrival ? "Arrival concept — controls" : "Notch concept — controls"
        window.isReleasedWhenClosed = false
        window.contentView = NSHostingView(rootView: PlaygroundPanel(state: services.state, actions: actions))
        if !window.setFrameUsingName("PlaygroundControls"), let screen = NSScreen.main {
            window.setFrameTopLeftPoint(CGPoint(x: screen.visibleFrame.minX + 24, y: screen.visibleFrame.maxY - 24))
        }
        window.setFrameAutosaveName("PlaygroundControls")
        window.orderFrontRegardless()             // shown without taking focus: the intro starts as on a real launch
        controls = window
    }
}

/// Where the notch sits (the product's own placement settings).
enum PlaygroundPlacement: Hashable {
    case notch
    case edge(FERBScreenEdge)

    static let all: [PlaygroundPlacement] = [.notch, .edge(.top), .edge(.right), .edge(.left), .edge(.bottom)]

    var label: String {
        switch self {
        case .notch: return "In the notch (docked)"
        case .edge(let edge): return "\(edge.rawValue.capitalized) edge"
        }
    }
}
#endif
