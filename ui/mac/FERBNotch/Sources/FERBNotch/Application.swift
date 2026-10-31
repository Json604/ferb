import AppKit
import Combine
import UserNotifications

@main
@MainActor
struct FERBNotchApp {
    static func main() {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        let delegate = AppDelegate()
        app.delegate = delegate
        withExtendedLifetime(delegate) { app.run() }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var coordinator: PanelCoordinator?
    private let appSetup = AppSetupController()
    private var ferbWindow: FERBWindowController?
    private var seenWindow: SeenWindowController?
    private var loginItems: LoginItemController?
    private var updates: UpdateCoordinator?
    private var sparkle: SparkleUpdater?
    private var updateOffer: AnyCancellable?
    private let poster = NotificationPoster(center: AppBundle.isApp ? UserNotificationCenter() as NotificationCenterPosting : nil)
    private let clicks = NotificationClicks()

    func applicationDidFinishLaunching(_ notification: Notification) {
        clicks.poster = poster
        if AppBundle.isApp { UNUserNotificationCenter.current().delegate = clicks }    // before launching finishes
        let menu = NSMenu()
        let appMenuItem = NSMenuItem()
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "Quit \(AppName.current)", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
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
        NSApp.mainMenu = menu
        coordinator = PanelCoordinator()
        coordinator?.poster = poster
        if appSetup.isPackaged {
            coordinator?.onSetUp = { [appSetup] in appSetup.runSetupFromMenu() }
        }
        appSetup.onFinished = { [weak self] in self?.coordinator?.showFromMenu() }   // Done: into the notch
        if let coordinator {
            var actions = FERBAppActions(feedback: coordinator.model.feedback, cursor: coordinator.cursor)
            actions.setHoverEnabled = { [weak model = coordinator.model] enabled in model?.setHoverEnabled(enabled) }
            let loginItems = LoginItemController.installed(available: appSetup.isPackaged)
            self.loginItems = loginItems
            actions.loginItems = loginItems
            // Uninstall unregisters macOS's login item first; `ferb uninstall` then removes the fallback file
            actions.beforeUninstall = { [loginItems, appSetup] in
                loginItems.removeForUninstall()
                appSetup.unregisterService()              // macOS stops listing FERB's background service
            }
            appSetup.onSettled = { [loginItems, weak coordinator] in
                loginItems.applyDefaultAfterSetup()
                coordinator?.model.noteUpdate()               // "Updated to <version>", once per new build
            }
            let updates = UpdateCoordinator(rpc: coordinator.model.rpc)
            if appSetup.isPackaged, let sparkle = SparkleUpdater(coordinator: updates) {
                updates.attach(sparkle)
                sparkle.start()
                self.sparkle = sparkle
                actions.updates = updates
            }
            self.updates = updates
            coordinator.onInstallUpdate = { [updates] in updates.checkNow() }
            updateOffer = updates.$status.sink { [weak coordinator] status in
                if case .available(let version) = status { coordinator?.updateOffer = version }
                else { coordinator?.updateOffer = nil }
            }
            if appSetup.isPackaged {
                let setup = appSetup.model
                actions.setup = setup
                actions.openSetup = { [appSetup] in appSetup.runSetupFromMenu() }
                actions.runFerb = { [setup] arguments, line, done in setup.runner.run(arguments, line: line, done: done) }
            }
            let window = FERBWindowController(store: FERBWindowStore(rpc: coordinator.model.rpc), actions: actions)
            coordinator.hasWindow = true                      // plan D's menu shows Open FERB Window
            coordinator.onOpenWindow = { [window] in window.show(.tasks) }
            coordinator.onOpenSettings = { [window] in window.show(.settings) }
            coordinator.model.onOpenMemory = { [window] source in window.showMemory(importedFrom: source) }
            let seen = SeenWindowController(rpc: coordinator.model.rpc)       // step 12e: What FERB saw
            coordinator.model.onShowSeen = { [seen] scope in seen.show(scope) }
            seenWindow = seen
            ferbWindow = window
        }
        coordinator?.start()
        appSetup.startAtLaunch()                 // packaged FERB.app only: the setup window or a quiet update
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        coordinator?.showFromMenu()
        return true
    }
    /// An update Sparkle will install on quit installs only with nothing running, or after the owner's Quit and
    /// Install (UpdateCoordinator.beforeQuit).
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard let updates, updates.installArmed, !updates.prepared else { return .terminateNow }
        updates.beforeQuit { allow in NSApp.reply(toApplicationShouldTerminate: allow) }
        return .terminateLater
    }
    func applicationWillTerminate(_ notification: Notification) {
        appSetup.stop()
        coordinator?.tearDown()
    }
}
