import AppKit
import Combine
import NotchCore
import UserNotifications

@main
@MainActor
struct FERBNotchApp {
    static func main() {
        #if FERB_PLAYGROUND
        // The intro playground (PLAYGROUND.md): debug builds only, and only under its own bundle id
        if PlaygroundMode.requested { PlaygroundMode.run() }
        #endif
        // A hearing mode (L26) runs and exits here, before NSApplication, the delegate or any window
        if let code = Hearing.run(CommandLine.arguments) { exit(code) }
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        let delegate = AppDelegate()
        app.delegate = delegate
        withExtendedLifetime(delegate) { app.run() }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSMenuItemValidation {
    private var coordinator: PanelCoordinator?
    private let appSetup = AppSetupController()
    private var ferbWindow: FERBWindowController?
    private let dock = DockPresence()
    private var loginItems: LoginItemController?
    private var updates: UpdateCoordinator?
    private var sparkle: SparkleUpdater?
    private var updateOffer: AnyCancellable?
    private var taskWatch: AnyCancellable?
    private let poster = NotificationPoster(center: AppBundle.isApp ? UserNotificationCenter() as NotificationCenterPosting : nil)
    private let clicks = NotificationClicks()
    /// Insomnia mode's cover windows (APP-980), the controller's surface; the packaged FERB.app only.
    private var cover: InsomniaCoverWindows?

    func applicationDidFinishLaunching(_ notification: Notification) {
        // APP-1110: opening FERB.app ends a turn-off from Settings, before anything reads or starts the service
        if appSetup.isPackaged { DaemonService.turnBackOn() }
        dock.start()
        if AppBundle.isApp {          // APP-601: the owner approved this copy; its nested code shouldn't be stopped later
            let bundle = Bundle.main.bundleURL
            Task.detached(priority: .utility) { OwnQuarantine.clear(bundle: bundle) }
            // APP-936: what earlier runs received for the composer goes once it is a week old (a task still running
            // across this restart keeps its file); run before anything can paste or drop one now
            ReceivedFiles().sweep()
        }
        clicks.poster = poster
        if AppBundle.isApp { UNUserNotificationCenter.current().delegate = clicks }    // before launching finishes
        let menu = NSMenu()
        let appMenuItem = NSMenuItem()
        let appMenu = NSMenu()
        // Command-Q closes the FERB window when it's in front (APP-654); Quit in the menu-bar menu always quits.
        let quit = NSMenuItem(title: "Quit \(AppName.current)", action: #selector(quitKey(_:)), keyEquivalent: "q")
        quit.target = self
        appMenu.addItem(quit)
        appMenuItem.submenu = appMenu
        menu.addItem(appMenuItem)
        let editItem = NSMenuItem()
        let edit = NSMenu(title: "Edit")
        for (title, selector, key) in [("Undo", "undo:", "z"), ("Redo", "redo:", "Z"),
                                        ("Cut", "cut:", "x"), ("Copy", "copy:", "c"),
                                        ("Paste", "paste:", "v"), ("Select All", "selectAll:", "a")] {
            edit.addItem(withTitle: title, action: Selector(selector), keyEquivalent: key)
        }
        editItem.submenu = edit
        menu.addItem(editItem)
        // Command-W and Command-M act on the window in front (FERB's window, the setup window), as in any Mac app.
        let windowItem = NSMenuItem()
        let windowMenu = NSMenu(title: "Window")
        windowMenu.addItem(withTitle: "Close", action: #selector(NSWindow.performClose(_:)), keyEquivalent: "w")
        windowMenu.addItem(withTitle: "Minimize", action: #selector(NSWindow.performMiniaturize(_:)), keyEquivalent: "m")
        windowItem.submenu = windowMenu
        menu.addItem(windowItem)
        NSApp.mainMenu = menu
        coordinator = PanelCoordinator()
        coordinator?.poster = poster
        // SENSE-420: FERB.app holds Location and tells the daemon the city; the exact spot only on a task's Yes
        if let model = coordinator?.model, AppBundle.isApp {
            model.deviceLocation = DeviceLocationReporter(rpc: model.rpc, source: SystemLocationSource.shared)
        }
        // FERB's one sound engine: the everyday sounds, the intro's tour and the banners' sound decision share it.
        let sound = FerbSoundEngine()
        coordinator?.model.feedback.player = sound
        poster.sounds = coordinator?.model.feedback
        appSetup.makeTourParts = { [sound] in (OrbController(sound: sound), sound) }
        if appSetup.isPackaged {
            coordinator?.onSetUp = { [appSetup] in appSetup.replayIntro() }   // Set Up FERB…: the intro again
            coordinator?.onRepair = { [appSetup] in appSetup.repairService() }   // Restart found no service
        }
        if let coordinator {
            var actions = FERBAppActions(feedback: coordinator.model.feedback, cursor: coordinator.cursor)
            actions.setHoverEnabled = { [weak model = coordinator.model] enabled in model?.setHoverEnabled(enabled) }
            actions.hotKeys = coordinator.hotKeys
            actions.turnDeleted = { [weak model = coordinator.model] request in model?.removeTurns(requestIDs: [request]) }
            actions.freshConversation = { [weak model = coordinator.model] in await model?.startFreshConversation() }
            // Settings → Phone: the link's helper is reached only through the daemon (S4, T-1204; kb PHN-441)
            actions.phoneLink = coordinator.model.rpc
            // APP-1110: Settings' Turn FERB off; the next launch starts everything as usual (turnBackOn, then setup)
            actions.runningTasks = { [weak model = coordinator.model] in model?.runningTasks.count ?? 0 }
            if appSetup.isPackaged {
                actions.turnOff = {
                    if await DaemonService().turnOff() { NSApp.terminate(nil); return }
                    let alert = NSAlert()
                    alert.messageText = TurnOffPlan.failed
                    _ = AppWindowPresentation.runModal(alert)
                }
            }
            let loginItems = LoginItemController.installed(available: appSetup.isPackaged)
            self.loginItems = loginItems
            actions.loginItems = loginItems
            // Uninstall unregisters macOS's login item first; `ferb uninstall` then removes the fallback file
            // DMN-598: before that, the service (Full Disk Access) removes FERB's files from the browsers' folders
            actions.removeBrowserFiles = { [appSetup] in await appSetup.removeBrowserFilesForUninstall() }
            // APP-986: the root helper for a closed Mac on battery, registered when the owner turns the mode on here
            let powerHelper = AppBundle.isApp ? PowerHelper(registrar: PowerHelperService()) { [rpc = coordinator.model.rpc] params in
                _ = try? await rpc.call("insomnia.report", params: params)
            } : nil
            coordinator.model.powerHelper = powerHelper
            actions.beforeUninstall = { [loginItems, appSetup, powerHelper] in
                loginItems.removeForUninstall()
                // insomnia mode's root helper first (APP-986), then the background service, each waited for and checked
                if let refused = await powerHelper?.unregisterForUninstall() { return refused }
                return await appSetup.unregisterService()  // macOS stops listing FERB's background service
            }
            appSetup.onSettled = { [loginItems, weak coordinator] in
                loginItems.applyDefaultAfterSetup()
                coordinator?.model.noteUpdate()               // "Updated to <version>", once per new build
            }
            let updates = UpdateCoordinator(rpc: coordinator.model.rpc, defaults: .standard)
            if appSetup.isPackaged, let sparkle = SparkleUpdater(coordinator: updates) {
                updates.attach(sparkle)
                sparkle.start()
                self.sparkle = sparkle
                actions.updates = updates
            }
            self.updates = updates
            coordinator.onInstallUpdate = { [updates] in updates.install() }
            // APP-980: the warning, then the mode; the cover only in the packaged app (its camera needs the bundle's
            // usage description)
            coordinator.onKeepWorking = { [weak model = coordinator.model] in
                if let model { InsomniaDialog.run(model, helper: powerHelper) }
            }
            if AppBundle.isApp {
                let cover = InsomniaCoverWindows(controller: coordinator.model.insomnia, rpc: coordinator.model.rpc,
                                                 access: coordinator.model.previewAccess)
                coordinator.model.insomnia.surface = cover
                self.cover = cover
            }
            // APP-947: Sparkle's window the owner was watching closes without an alert; FERB says it in the chat
            updates.onInstallFailed = { [weak model = coordinator.model] line in
                model?.noteFromSetup(id: "update-failed-\(Int(Date().timeIntervalSince1970))", text: line)
            }
            updateOffer = updates.$status.sink { [weak coordinator] status in
                guard let coordinator else { return }
                switch status {
                case .available(let version): (coordinator.updateOffer, coordinator.updateAsks) = (version, false)
                case .asking(let version):                     // the Developer ID switch asks first (APP-903)
                    (coordinator.updateOffer, coordinator.updateAsks) = (version, true)
                    coordinator.model.noteUpdateAsk(version: version)
                default: (coordinator.updateOffer, coordinator.updateAsks) = (nil, false)
                }
            }
            if appSetup.isPackaged {
                let setup = appSetup.model
                // APP-921: a restart a running task deferred is said once in the chat and made when no task runs
                setup.tell = { [weak model = coordinator.model] id, line in model?.noteFromSetup(id: id, text: line) }
                // a temporary chat's task is running work too (APP-1125): a restart waits for it
                taskWatch = coordinator.model.$runningTasks.map { !$0.isEmpty }
                    .combineLatest(coordinator.model.$connectionStatus.map { $0 == "Connected to FERB" })
                    .removeDuplicates { $0 == $1 }
                    .sink { [setup] running, connected in
                        guard connected else { return }
                        Task { @MainActor in await setup.tasksChanged(running: running) }
                    }
                actions.setup = setup
                actions.replayIntro = { [appSetup] in appSetup.replayIntro() }
                actions.runFerb = { [setup] arguments, line, done in setup.runner.run(arguments, line: line, done: done) }
            }
            actions.correctMemory = { [weak coordinator] text in
                coordinator?.model.prepareMemoryCorrection(text)
                coordinator?.showFromMenu()
            }
            let store = FERBWindowStore(rpc: coordinator.model.rpc)
            store.memoryReview = coordinator.model.memoryReview          // MEM-460: one review, chat and Memory page
            let window = FERBWindowController(store: store, actions: actions)
            coordinator.hasWindow = true                      // plan D's menu shows Open FERB Window
            coordinator.onOpenWindow = { [window] in window.show(.tasks) }
            coordinator.onOpenSettings = { [window] in window.show(.settings) }
            coordinator.model.onOpenMemory = { [window] source in window.showMemory(importedFrom: source) }
            coordinator.model.onOpenPhone = { [window] in window.show(.settings, tab: .phone) }
            ferbWindow = window
        }
        appSetup.notch = coordinator?.model
        appSetup.introControls = coordinator?.introControls
        coordinator?.start()
        AppWindowPresentation.start()            // from launch: a window that blocks FERB is never hidden (APP-948)
        appSetup.startAtLaunch()                // packaged FERB.app only: the intro or a quiet update
    }

    @objc func quitKey(_ sender: Any?) {
        if ferbWindow?.closeOnQuitKey(keyWindow: NSApp.keyWindow) == true { return }
        NSApp.terminate(sender)
    }

    /// The app menu says what Command-Q does now.
    func validateMenuItem(_ menuItem: NSMenuItem) -> Bool {
        if menuItem.action == #selector(quitKey(_:)) {
            let closes = ferbWindow?.window.map { $0 === NSApp.keyWindow && $0.isVisible } ?? false
            menuItem.title = closes ? "Close \(AppName.current) Window" : "Quit \(AppName.current)"
        }
        return true
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        coordinator?.showFromMenu()
        return true
    }
    /// An update Sparkle will install on quit installs only with nothing running, or after the owner's Quit and
    /// Install (UpdateCoordinator.beforeQuit).
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        PermissionRelaunchWatcher.noteQuit()        // System Settings' Quit & Reopen: FERB comes back (APP-689)
        guard let updates, updates.installArmed, !updates.prepared else { return .terminateNow }
        updates.beforeQuit { allow in NSApp.reply(toApplicationShouldTerminate: allow) }
        return .terminateLater
    }
    func applicationWillTerminate(_ notification: Notification) {
        PermissionRelaunchWatcher.startIfNoted(poll: appSetup.model.grantPoll, reads: appSetup.model.serviceStartAfter)
        appSetup.stop()
        coordinator?.model.keepUnsavedHistory()   // chat entries not written yet wait in the outbox (APP-805)
        coordinator?.tearDown()
    }
}
