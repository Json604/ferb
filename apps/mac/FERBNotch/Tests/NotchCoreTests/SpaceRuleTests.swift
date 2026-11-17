import AppKit
import XCTest
@testable import FERBNotch

/// Owner, 2026-11-08 10:59 (build 202611080329): "when settings window is open, then click on menu bar icon of ferb it
/// deos open its dialog on any other space excpet the home space ... also why shift space to oipen ferb windows??? why
/// cant it be opeend on teh same space???". One rule for every way FERB shows a window or a menu: it appears on the
/// Space the owner is on, and FERB never becomes the active app in a way that switches Spaces. The window server, the
/// Spaces, the Dock policy and activation are stand-ins here: no window is ever put on screen.
@MainActor
final class SpaceRuleTests: XCTestCase {
    /// The owner's Space now, FERB's "own" Space where macOS put its new windows and menus (3, in the logs), and the
    /// Space where the owner left FERB's window open.
    private let here: UInt64 = 2896, home: UInt64 = 3, leftOpen: UInt64 = 1380
    private var spaces: [ObjectIdentifier: [UInt64]] = [:]
    private var showing: Set<ObjectIdentifier> = []
    private var windows: [NSWindow] = []
    private var menus: [NSWindow] = []
    private var events: [String] = []
    private var policy: NSApplication.ActivationPolicy = .accessory
    private var active = false
    private var clock = Date(timeIntervalSinceReferenceDate: 0)

    override func setUp() async throws {
        _ = NSApplication.shared
        let P = AppWindowPresentation.self
        let saved = (P.spacesOf, P.activeSpace, P.move, P.orderIn, P.makeKey, P.activate, P.isActive, P.policy,
                     P.setPolicy, P.isShowing, P.visibleWindows, P.now, P.runAlert, P.runPanel, P.menuWindows)
        P.spacesOf = { [unowned self] in self.spaces[ObjectIdentifier($0)] ?? [] }
        P.activeSpace = { [unowned self] in self.here }
        P.move = { [unowned self] window in
            self.spaces[ObjectIdentifier(window)] = [self.here]
            self.events.append("move")
        }
        // macOS puts a window FERB orders in on FERB's own Space, not the owner's (the bug the rule answers)
        P.orderIn = { [unowned self] window in self.appear(window, on: [self.home]); self.events.append("order in") }
        P.makeKey = { [unowned self] _ in self.events.append("key") }
        P.activate = { [unowned self] in self.events.append("activate"); self.active = true }
        P.isActive = { [unowned self] in self.active }
        P.policy = { [unowned self] in self.policy }
        P.setPolicy = { [unowned self] in self.policy = $0; self.events.append("dock \($0 == .regular)") }
        P.isShowing = { [unowned self] in self.showing.contains(ObjectIdentifier($0)) }
        P.visibleWindows = { [unowned self] in self.windows.filter { self.showing.contains(ObjectIdentifier($0)) } }
        P.menuWindows = { [unowned self] in self.menus.filter { self.showing.contains(ObjectIdentifier($0)) } }
        P.now = { [unowned self] in self.clock }
        P.start()                                         // the menu tracking observers, as FERB's menu bar icon starts them
        addTeardownBlock { @MainActor [unowned self] in
            self.clock += 3600                            // the moment ends while the stand-ins are still in place
            P.tick()
            (P.spacesOf, P.activeSpace, P.move, P.orderIn, P.makeKey, P.activate, P.isActive, P.policy, P.setPolicy,
             P.isShowing, P.visibleWindows, P.now, P.runAlert, P.runPanel, P.menuWindows) = saved
        }
    }

    private func window(_ kind: AnyClass = NSWindow.self, styleMask: NSWindow.StyleMask = [.titled, .closable],
                        level: NSWindow.Level = .normal) -> NSWindow {
        let made: NSWindow = kind == NSPanel.self
            ? NSPanel(contentRect: NSRect(x: 0, y: 0, width: 300, height: 200), styleMask: styleMask, backing: .buffered,
                      defer: true)
            : (kind == FERBAppWindow.self
                ? FERBAppWindow(contentRect: NSRect(x: 0, y: 0, width: 300, height: 200), styleMask: styleMask,
                                backing: .buffered, defer: true)
                : NSWindow(contentRect: NSRect(x: 0, y: 0, width: 300, height: 200), styleMask: styleMask,
                           backing: .buffered, defer: true))
        made.isReleasedWhenClosed = false
        made.level = level
        return made
    }

    private func appear(_ window: NSWindow, on where: [UInt64]) {
        if !windows.contains(where: { $0 === window }) { windows.append(window) }
        showing.insert(ObjectIdentifier(window))
        spaces[ObjectIdentifier(window)] = `where`
    }

    private func isHere(_ window: NSWindow) -> Bool { !AppWindowPresentation.isElsewhere(window) }

    /// The moment's watch, as its timer would run it, until the moment is over.
    private func endMoment() {
        AppWindowPresentation.tick()
        clock += AppWindowPresentation.momentLength
        AppWindowPresentation.tick()
    }

    /// FERB's Settings window left open on another Space (the owner's report): it must stay there through every path.
    @discardableResult
    private func settingsLeftOpen() -> NSWindow {
        let settings = window(FERBAppWindow.self)
        appear(settings, on: [leftOpen])
        policy = .regular                                  // FERB is in the Dock while it's open (APP-600)
        return settings
    }

    // decision: APP-796
    func testEveryPathEndsOnTheOwnersSpaceWithoutASpaceSwitchingActivation() {
        let paths = AppWindowPresentation.paths
        XCTAssertFalse(paths.isEmpty)
        for path in paths {
            for withSettingsOpen in [false, true] {
                setUpWorld(withSettingsOpen)
                let settings = withSettingsOpen ? settingsLeftOpen() : nil
                let shown = open(path)
                let label = "\(path.name)\(withSettingsOpen ? ", Settings open on another Space" : "")"
                XCTAssertTrue(isHere(shown), "\(label): on the owner's Space")
                if let settings {
                    XCTAssertEqual(spaces[ObjectIdentifier(settings)], [leftOpen],
                                   "\(label): the window left open elsewhere stays where it is")
                }
                // FERB becomes the active app only when it already was one or was already in the Dock: never as it
                // joins the Dock (the Dock then switched Spaces, 6 of 6 times in the owner's logs)
                if events.contains("activate") {
                    XCTAssertTrue(withSettingsOpen, "\(label): FERB activated while it wasn't in the Dock")
                    XCTAssertFalse(events.contains("dock true"), "\(label): FERB activated as it joined the Dock")
                }
                if path.mechanism == .everySpace || path.mechanism == .menu {
                    XCTAssertFalse(events.contains("activate"), "\(label): showing it never activates FERB")
                }
                endMoment()
            }
        }
    }

    private func setUpWorld(_ settingsOpen: Bool) {
        spaces = [:]; showing = []; windows = []; menus = []; events = []
        policy = .accessory; active = false
        clock += 60
        AppWindowPresentation.tick()
    }

    /// Opens one path the way FERB does, with the window server placing what it shows: new windows and menus land on
    /// FERB's own Space (3), never the owner's; the overlay and all-Spaces windows are on every Space or in the overlay.
    private func open(_ path: AppWindowPresentation.Path) -> NSWindow {
        switch path.mechanism {
        case .window:
            let ferb = window(FERBAppWindow.self)
            AppWindowPresentation.show(ferb)
            XCTAssertTrue(ferb.collectionBehavior.contains(.moveToActiveSpace), path.name)
            XCTAssertTrue(ferb.collectionBehavior.contains(.fullScreenAuxiliary),
                          "\(path.name): allowed over a full-screen app")
            if let order = events.firstIndex(of: "order in"), let dock = events.firstIndex(of: "dock true") {
                XCTAssertLessThan(dock, order, "\(path.name): FERB joins the Dock before its window shows")
            }
            return ferb
        case .opening:
            let opened = window(NSPanel.self)
            if path.code == "NSAlert()" {
                AppWindowPresentation.runAlert = { [unowned self] _ in
                    self.appear(opened, on: [self.home])        // runModal orders the alert in, then the watch ticks
                    AppWindowPresentation.tick()
                    return .alertFirstButtonReturn
                }
                _ = AppWindowPresentation.runModal(NSAlert())
            } else if path.code == "NSOpenPanel()", path.file == "WindowPages.swift" {
                AppWindowPresentation.runPanel = { [unowned self] _ in
                    self.appear(opened, on: [self.home])
                    AppWindowPresentation.tick()
                    return .cancel
                }
                _ = AppWindowPresentation.runModal(NSOpenPanel())
            } else {
                AppWindowPresentation.opening(path.name) { appear(opened, on: [home]) }   // shows a moment later
                AppWindowPresentation.tick()
            }
            return opened
        case .menu:
            let menuWindow = window(NSPanel.self, styleMask: [.borderless], level: .popUpMenu)
            menus.append(menuWindow)
            if path.code == "statusItem(withLength" {
                AppWindowPresentation.menuOpening()           // the status item's menuWillOpen
            } else {
                AppWindowPresentation.pressed()               // the press that opens a picker, Menu or context menu
            }
            appear(menuWindow, on: [home])
            let menu = NSMenu()
            NotificationCenter.default.post(name: NSMenu.didBeginTrackingNotification, object: menu)
            XCTAssertTrue(AppWindowPresentation.watching, "\(path.name): watched while it tracks")
            AppWindowPresentation.tick()
            NotificationCenter.default.post(name: NSMenu.didEndTrackingNotification, object: menu)
            return menuWindow
        case .everySpace:
            let panel = window(NSPanel.self, styleMask: [.borderless, .nonactivatingPanel])
            appear(panel, on: [])                            // in FERB's overlay space: on no ordinary Space
            AppWindowPresentation.pressed()
            AppWindowPresentation.tick()
            XCTAssertFalse(events.contains("move"), "\(path.name): never moved")
            appear(panel, on: [home, leftOpen, here])         // the AppKit fallback: on every Space
            AppWindowPresentation.tick()
            XCTAssertFalse(events.contains("move"), "\(path.name): never moved")
            return panel
        case .withParent:
            let parent = window(FERBAppWindow.self)
            AppWindowPresentation.show(parent)
            let child = window(NSPanel.self, styleMask: [.borderless])
            parent.addChildWindow(child, ordered: .above)
            appear(child, on: spaces[ObjectIdentifier(parent)] ?? [])     // a child shows where its window is
            AppWindowPresentation.tick()
            parent.removeChildWindow(child)
            return child
        }
    }

    /// The menu bar icon's menu with FERB's window open on another Space: the menu comes to the Space showing, and the
    /// window stays where the owner left it.
    // decision: APP-796
    func testTheMenuBarIconsMenuOpensOnEverySpaceWithSettingsOpenElsewhere() {
        let settings = settingsLeftOpen()
        for _ in 0..<3 {
            let menuWindow = window(NSPanel.self, styleMask: [.borderless], level: .popUpMenu)
            menus = [menuWindow]
            AppWindowPresentation.menuOpening()
            appear(menuWindow, on: [home])                   // where macOS put it (another Space)
            AppWindowPresentation.tick()
            XCTAssertTrue(isHere(menuWindow))
            XCTAssertTrue(AppWindowPresentation.watching, "kept here while it is open")
            endMoment()
            XCTAssertFalse(AppWindowPresentation.watching)
            showing.remove(ObjectIdentifier(menuWindow))
        }
        XCTAssertEqual(spaces[ObjectIdentifier(settings)], [leftOpen])
        XCTAssertFalse(events.contains("activate"))
    }

    /// Opening FERB's window from another Space: it joins the Dock first, comes here, and FERB doesn't activate in the
    /// same moment; opened again while FERB is in the Dock it activates (the logs' no-switch case) and still comes here.
    // decision: APP-796, APP-688, APP-773, APP-600
    func testFERBsWindowComesHereAndActivatesOnlyWhenThatCantSwitchSpaces() {
        let ferb = window(FERBAppWindow.self)
        AppWindowPresentation.show(ferb)
        XCTAssertEqual(events, ["dock true", "order in", "move", "key"])
        XCTAssertTrue(isHere(ferb))
        endMoment()

        // left open on another Space, asked for again: it comes here before anything orders it front
        events = []
        spaces[ObjectIdentifier(ferb)] = [leftOpen]
        AppWindowPresentation.show(ferb)
        XCTAssertEqual(events.first, "move")
        XCTAssertTrue(events.contains("activate"), "already in the Dock: activating switched nothing in the logs")
        XCTAssertTrue(isHere(ferb))
        endMoment()
    }

    /// Owner 2026-11-09: "settings drag is still not fixed. but i checked it a step further this time and thsi
    /// stickiness to one side only happens when window is opened in a space alerayd ackquired … when i opened settings
    /// in home i was able to drag it anywheer." A FERB window the rule shows on the owner's Space (here 2896,
    /// full-screen VS Code in the logs) is that Space's auxiliary window, so the window server lets the owner drag it;
    /// it still joins only the Space showing. A window AppKit puts elsewhere (an alert) comes here the same way; menus
    /// are left as AppKit makes them.
    // decision: APP-804, APP-796
    func testAWindowShownOnTheOwnersSpaceIsAllowedOverAFullScreenApp() {
        let ferb = window(FERBAppWindow.self)
        AppWindowPresentation.show(ferb)
        XCTAssertTrue(isHere(ferb))
        XCTAssertTrue(ferb.collectionBehavior.isSuperset(of: [.fullScreenAuxiliary, .moveToActiveSpace]))
        XCTAssertFalse(ferb.collectionBehavior.contains(.canJoinAllSpaces), "it stays on the one Space it opened on")
        endMoment()

        let alert = window(NSPanel.self)
        AppWindowPresentation.opening("an alert") { appear(alert, on: [home]) }
        AppWindowPresentation.tick()
        XCTAssertTrue(isHere(alert))
        XCTAssertTrue(alert.collectionBehavior.contains(.fullScreenAuxiliary),
                      "brought here, allowed over a full-screen app")
        endMoment()

        let menuWindow = window(NSPanel.self, styleMask: [.borderless], level: .popUpMenu)
        menus = [menuWindow]
        AppWindowPresentation.menuOpening()
        appear(menuWindow, on: [home])
        AppWindowPresentation.tick()
        XCTAssertTrue(isHere(menuWindow))
        XCTAssertFalse(menuWindow.collectionBehavior.contains(.fullScreenAuxiliary),
                       "a menu is left as AppKit makes it")
        endMoment()
    }

    /// A Space switch in the moment after showing (one FERB's activation might still make) takes the window along to
    /// where the owner landed; a later switch is the owner's own and leaves it.
    // decision: APP-796
    func testAWindowShownJustNowGoesWhereTheOwnerLands() {
        policy = .regular
        let ferb = window(FERBAppWindow.self)
        AppWindowPresentation.show(ferb)
        XCTAssertTrue(isHere(ferb))
        spaces[ObjectIdentifier(ferb)] = [leftOpen]           // the Space showing changed under it
        AppWindowPresentation.tick()
        XCTAssertTrue(isHere(ferb))
        endMoment()
        spaces[ObjectIdentifier(ferb)] = [leftOpen]
        AppWindowPresentation.tick()
        XCTAssertFalse(isHere(ferb), "after the moment the owner's own switch leaves it")
    }

    /// Joining the Dock waits while FERB is the active app (the Dock took a join then as a launch with no windows);
    /// leaving it doesn't.
    // decision: APP-796, APP-600
    func testFERBJoinsTheDockOnlyWhileItIsntTheActiveApp() {
        XCTAssertEqual(DockPresence.change(wants: true, current: .accessory, active: false), .regular)
        XCTAssertNil(DockPresence.change(wants: true, current: .accessory, active: true))
        XCTAssertNil(DockPresence.change(wants: true, current: .regular, active: true))
        XCTAssertEqual(DockPresence.change(wants: false, current: .regular, active: true), .accessory)
        XCTAssertNil(DockPresence.change(wants: false, current: .accessory, active: false))
    }

    /// A new window type or presenter must be listed in the rule's paths: the build lists every source line that puts a
    /// window or menu on screen (SettingsKeyScan → SourcePresenters), and each needs a path with its file and code.
    /// Only the rule makes FERB the active app or runs a modal window.
    // decision: APP-796
    func testEveryWindowAndMenuInTheSourcesIsAPathOfTheRule() {
        let paths = AppWindowPresentation.paths
        XCTAssertFalse(SourcePresenters.lines.isEmpty)
        for line in SourcePresenters.lines {
            let file = String(line.prefix { $0 != ":" })
            XCTAssertTrue(paths.contains { $0.file == file && line.contains($0.code) },
                          "not in AppWindowPresentation.paths: \(line)")
        }
        for path in paths {
            XCTAssertTrue(SourcePresenters.lines.contains { $0.hasPrefix(path.file + ":") && $0.contains(path.code) },
                          "a path no source line opens any more: \(path.name)")
        }
        for line in SourcePresenters.activations {
            XCTAssertTrue(line.hasPrefix("AppWindowPresentation.swift:"),
                          "only the Space rule activates FERB or runs a modal window: \(line)")
        }
        // APP-946: a plain Check for Updates… is Sparkle's probing check; Sparkle's own window (and the standard
        // driver's modal alerts that come with a user-initiated check) opens only for the owner's Install
        XCTAssertFalse(SourcePresenters.lines.contains { $0.contains("controller") && $0.contains("checkForUpdates(") },
                       "no source calls controller.checkForUpdates( for a plain check")
        let sparkleWindows = SourcePresenters.lines.filter { $0.contains("checkForUpdates(") }
        XCTAssertEqual(sparkleWindows.count, 1, "\(sparkleWindows)")
        for line in sparkleWindows {
            XCTAssertTrue(line.hasPrefix("SparkleUpdater.swift:") && line.contains("opening(\"the update window\""),
                          "Sparkle's window opens only through the rule, for the owner's Install: \(line)")
        }
    }

    // MARK: - A window that blocks FERB.app (APP-948, APP-949)

    /// What the rule left on a modal alert AppKit ran by itself, read while it runs.
    private struct Raised {
        var level: NSWindow.Level
        var behavior: NSWindow.CollectionBehavior
        var here: Bool
        var events: [String]
        var quitEnabled: Bool
        var otherEnabled: Bool
    }

    /// Runs a real NSAlert with plain AppKit (`runModal`, never through AppWindowPresentation), with FERB's Settings
    /// window at `settingsLevel`; reads the rule's work at 0.3 s, then the owner changes Space, then at 0.8 s it ends.
    private func runPlainAlert(settingsLevel: NSWindow.Level) -> (Raised?, afterSwitch: [String]) {
        let app = NSApplication.shared
        let wasProhibited = app.activationPolicy() == .prohibited
        if wasProhibited { app.setActivationPolicy(.accessory) }
        defer { if wasProhibited { app.setActivationPolicy(.prohibited) } }
        let settings = window(FERBAppWindow.self, level: settingsLevel)
        appear(settings, on: [here])
        let menu = NSMenu()
        let quit = NSMenuItem(title: "Quit FERB", action: nil, keyEquivalent: "")
        ModalSafeQuit.shared.take(quit)
        let target = PlainMenuTarget()
        let other = NSMenuItem(title: "Open FERB", action: #selector(PlainMenuTarget.open(_:)), keyEquivalent: "")
        other.target = target
        menu.addItem(quit)
        menu.addItem(other)
        var raised: Raised?
        var afterSwitch: [String] = []
        let look = Timer(timeInterval: 0.3, repeats: false) { [unowned self] _ in
            MainActor.assumeIsolated {
                guard let modal = NSApp.modalWindow else { return }
                menu.update()
                raised = Raised(level: modal.level, behavior: modal.collectionBehavior, here: self.isHere(modal),
                                events: self.events, quitEnabled: quit.isEnabled, otherEnabled: other.isEnabled)
                self.events = []
                self.spaces[ObjectIdentifier(modal)] = [self.leftOpen]  // the owner went to another Space
            }
        }
        let end = Timer(timeInterval: 0.8, repeats: false) { [unowned self] _ in
            MainActor.assumeIsolated {
                afterSwitch = self.events
                if let modal = NSApp.modalWindow { XCTAssertTrue(self.isHere(modal), "it followed the owner") }
                NSApp.stopModal(withCode: .cancel)
            }
        }
        let safety = Timer(timeInterval: 5, repeats: false) { _ in NSApp.abortModal() }
        for timer in [look, end, safety] { RunLoop.main.add(timer, forMode: .modalPanel) }
        let alert = NSAlert()
        alert.messageText = "A framework's alert"
        _ = alert.runModal()                                  // plain AppKit, as Sparkle's standard driver ran it
        safety.invalidate()
        return (raised, afterSwitch)
    }

    /// Owner, 2026-11-17: Sparkle's error alert ran modally where he couldn't see it; FERB took no clicks and every
    /// item of its menu was greyed out, Quit included. Whoever runs a modal window, it comes to the owner's Space
    /// (allowed over a full-screen app), above FERB's own windows, ordered in and made key; FERB becomes the active app
    /// for it because the owner had just pressed in FERB and FERB is in the Dock; it follows a Space change; Quit stays.
    // decision: APP-945, APP-948, APP-949
    func testAModalAlertRunByPlainAppKitIsRaisedWhereTheOwnerIs() throws {
        XCTAssertTrue(AppWindowPresentation.watchesBlocking)
        policy = .regular                                     // Settings is open: FERB is in the Dock
        AppWindowPresentation.pressed()                       // the owner's press on Check for Updates…
        let (raised, afterSwitch) = runPlainAlert(settingsLevel: NSWindow.Level(rawValue: 20))
        let seen = try XCTUnwrap(raised, "the alert ran modally")
        XCTAssertGreaterThan(seen.level.rawValue, 20, "above FERB's own windows")
        XCTAssertGreaterThanOrEqual(seen.level.rawValue, NSWindow.Level.modalPanel.rawValue)
        XCTAssertLessThan(seen.level.rawValue, NSWindow.Level.popUpMenu.rawValue, "menus still show above it")
        XCTAssertTrue(seen.behavior.isSuperset(of: [.moveToActiveSpace, .fullScreenAuxiliary]))
        XCTAssertFalse(seen.behavior.contains(.canJoinAllSpaces), "one Space at a time (APP-804)")
        XCTAssertTrue(seen.here, "on the owner's Space")
        XCTAssertTrue(seen.events.contains("move"), "macOS put it elsewhere; the rule brought it here")
        XCTAssertEqual(seen.events.filter { $0 == "activate" }.count, 1, "the owner just asked: FERB comes forward")
        XCTAssertTrue(seen.events.contains("key"))
        XCTAssertTrue(seen.quitEnabled, "Quit works while a modal window runs")
        XCTAssertFalse(seen.otherEnabled, "AppKit greys the rest, as it did for the owner")
        XCTAssertTrue(afterSwitch.contains("move"), "a Space change while it is open brings it along")
        XCTAssertFalse(afterSwitch.contains("activate"), "only its first look may activate FERB")
    }

    /// The same alert with no press of the owner's just before it (a framework's own timing): it still comes here,
    /// above FERB's windows, and is made key within FERB, but FERB doesn't become the active app (APP-797, rule 10).
    // decision: APP-948, APP-797
    func testAModalAlertNobodyAskedForShowsWithoutActivatingFERB() throws {
        clock += AppWindowPresentation.askedLength + 60
        policy = .regular
        let (raised, _) = runPlainAlert(settingsLevel: .normal)
        let seen = try XCTUnwrap(raised)
        XCTAssertTrue(seen.here)
        XCTAssertGreaterThanOrEqual(seen.level.rawValue, NSWindow.Level.modalPanel.rawValue)
        XCTAssertFalse(seen.events.contains("activate"))
        XCTAssertTrue(seen.events.contains("key"))
    }

    /// The pieces: a modal window's level and a window off every display.
    // decision: APP-948
    func testABlockingWindowsLevelAndPlace() {
        let P = AppWindowPresentation.self
        XCTAssertEqual(P.blockingLevel(current: .normal, others: []), .modalPanel)
        XCTAssertEqual(P.blockingLevel(current: .modalPanel, others: [.normal, .floating]), .modalPanel)
        XCTAssertEqual(P.blockingLevel(current: .modalPanel, others: [NSWindow.Level(rawValue: 30)]).rawValue, 31)
        XCTAssertEqual(P.blockingLevel(current: .modalPanel, others: [NSWindow.Level(rawValue: 200)]).rawValue,
                       NSWindow.Level.popUpMenu.rawValue - 1)
        let screen = CGRect(x: 0, y: 0, width: 1440, height: 900)
        XCTAssertNil(P.onScreen(CGRect(x: 100, y: 100, width: 260, height: 140), screens: [screen], main: screen))
        XCTAssertEqual(P.onScreen(CGRect(x: 5000, y: 100, width: 260, height: 140), screens: [screen], main: screen),
                       CGRect(x: 590, y: 380, width: 260, height: 140))
    }
}

/// A menu item's target that, like most of FERB's, doesn't work while a modal window runs.
private final class PlainMenuTarget: NSObject {
    @objc func open(_ sender: Any?) {}
}
