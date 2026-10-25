import AppKit
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
    private var loginItems: LoginItemController?
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
            let loginItems = LoginItemController.installed(available: appSetup.isPackaged)
            self.loginItems = loginItems
            actions.loginItems = loginItems
            // Uninstall unregisters macOS's login item first; `ferb uninstall` then removes the fallback file
            actions.beforeUninstall = { [loginItems] in loginItems.removeForUninstall() }
            appSetup.onSettled = { [loginItems] in loginItems.applyDefaultAfterSetup() }
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
    func applicationWillTerminate(_ notification: Notification) {
        appSetup.stop()
        coordinator?.tearDown()
    }
}
