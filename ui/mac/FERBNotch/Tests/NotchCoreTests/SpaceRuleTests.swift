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
    }
}
