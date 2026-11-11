import AppKit
import Darwin
import os

/// FERB's one Space rule (APP-796; owner, 2026-11-08: "why shift space to oipen ferb windows??? why cant it be opeend
/// on teh same space???"): a FERB window or menu appears on the Space the owner is on now, and FERB never switches the
/// owner's Space. Every way FERB shows a window or a menu goes through here; `Path` lists them all, and a test fails on
/// a window class or presenter in the sources that isn't listed.
///
/// What switched Spaces (the owner's logs, 2026-11-07 and 08, every fresh open of FERB's window, 6 of 6): FERB became
/// the active app for its window and, a moment later, joined the Dock (APP-600, DockPresence). The Dock took that as
/// the launch of the front app "with no windows" ("app ordered front with no windows for pid …") and switched the
/// owner to Space 3 within 0.4 s. When FERB was already in the Dock (2 of 2), showing the window and activating switched
/// nothing. So FERB joins the Dock first, while it isn't the active app, and becomes the active app only when it
/// already was one or was already in the Dock (`mayActivate`).
///
/// What hid menus: the window server put FERB's new menus on Space 3, out of sight (logged for APP-790's picker; for the
/// menu bar icon's menu with Settings open on Space 3, 2026-11-08 10:58, the menu's window was made and nothing showed
/// on Space 2896, and its Space is inferred, not logged). The APP-790 watch moved only menus opened from FERB's window,
/// never the status item's. Whatever opened it, a FERB window or menu
/// the window server has only on other Spaces when it opens is moved to the Space showing (`bringHere`); one in FERB's
/// overlay space or on every Space (the notch, the sticky window) is already here and is never touched.
@MainActor
enum AppWindowPresentation {
    // MARK: - The window server and FERB's app state (tests stand in their own and never put a window on screen)

    /// The ordinary Spaces the window server has a window on: none for a window in FERB's overlay space or ordered out.
    static var spacesOf: (NSWindow) -> [UInt64] = { WindowServerSpaces.spaces(ofWindow: $0.windowNumber) }
    /// The Space showing (on the display with the menu bar's focus).
    static var activeSpace: () -> UInt64? = { WindowServerSpaces.activeSpace() }
    /// Moves one of FERB's windows to the Space showing; it never changes which Space shows.
    static var move: (NSWindow) -> Void = { moveToActiveSpace($0) }
    static var orderIn: (NSWindow) -> Void = { $0.orderFrontRegardless() }
    static var makeKey: (NSWindow) -> Void = { window in if window.canBecomeKey { window.makeKey() } }
    /// FERB becoming the active app; only `mayActivate` lets it happen.
    static var activate: () -> Void = { NSApp.activate(ignoringOtherApps: true) }
    static var isActive: () -> Bool = { NSApp.isActive }
    static var policy: () -> NSApplication.ActivationPolicy = { NSApp.activationPolicy() }
    static var setPolicy: (NSApplication.ActivationPolicy) -> Void = { _ = NSApp.setActivationPolicy($0) }
    /// FERB's windows on screen now (the moment's watch reads these; tests stand in their own).
    static var isShowing: (NSWindow) -> Bool = { $0.isVisible }
    static var visibleWindows: () -> [NSWindow] = { NSApp.windows.filter(\.isVisible) }
    static var now: () -> Date = { Date() }
    /// AppKit's own modal runs, behind `runModal` (tests stand in their own).
    static var runAlert: (NSAlert) -> NSApplication.ModalResponse = { $0.runModal() }
    static var runPanel: (NSSavePanel) -> NSApplication.ModalResponse = { $0.runModal() }

    /// The sticky window's frame while window mode shows it (PanelCoordinator sets it): a window brought on screen
    /// opens beside it rather than over or under it, where the screen has room (APP-774).
    static var avoid: () -> CGRect? = { nil }

    // MARK: - The rule

    /// Whether the window server has the window only on Spaces other than the one showing. A window on no ordinary Space
    /// (in FERB's overlay space, or ordered out), on every Space, or in its own full-screen Space is never elsewhere.
    static func isElsewhere(_ window: NSWindow) -> Bool {
        guard !window.styleMask.contains(.fullScreen) else { return false }
        let spaces = spacesOf(window)
        guard !spaces.isEmpty, let active = activeSpace() else { return false }
        return !spaces.contains(active)
    }

    /// The rule's one move: a FERB window or menu that is elsewhere comes to the Space showing. True when it moved. A
    /// window (not a menu) comes allowed over a full-screen app (`mayShowOverFullScreen`).
    @discardableResult
    static func bringHere(_ window: NSWindow, _ what: String) -> Bool {
        guard isElsewhere(window) else { return false }
        if window.level < .popUpMenu { mayShowOverFullScreen(window) }
        move(window)
        log.notice("FERB \(what, privacy: .public) \(window.windowNumber, privacy: .public) was on another Space: brought to the Space showing")
        return true
    }

    /// A FERB window the rule shows on the owner's Space may be there over a full-screen app, as that Space's auxiliary
    /// window (APP-804). Without it, FERB's window put on a full-screen app's Space showed but none of the owner's
    /// title-bar drags began (2026-11-09, over full-screen VS Code: every press logged the window server's "no client
    /// initiated window drag trait gesture" and AppKit's "Window move completed without beginning"); opened on a
    /// desktop it dragged. It still joins only the Space showing (`moveToActiveSpace`), never every Space.
    static func mayShowOverFullScreen(_ window: NSWindow) {
        window.collectionBehavior.insert(.fullScreenAuxiliary)
    }

    /// Whether FERB may become the active app now without the Dock switching the owner's Space: it already is the
    /// active app, or it was already in the Dock before what it shows was asked for.
    static func mayActivate(inDockBefore: Bool) -> Bool { isActive() || inDockBefore }

    /// FERB joins the Dock for a titled window (APP-600) before the window shows, by DockPresence's rule (never while
    /// FERB is the active app), rather than after the window became key.
    static func joinDock(for window: NSWindow) {
        let counts = DockPresence.wantsDock([.init(isPanel: window is NSPanel, isTitled: window.styleMask.contains(.titled),
                                                   isVisible: true, isMiniaturized: false)])
        if let next = DockPresence.change(wants: counts, current: policy(), active: isActive()), next == .regular {
            setPolicy(next)
        }
    }

    // MARK: - FERB's own titled windows (Settings and every page of FERB's window)

    static func show(_ window: NSWindow) {
        start()
        // Ordering it in or making it key brings the window to the owner's Space instead of switching Spaces (APP-688).
        window.collectionBehavior.insert(.moveToActiveSpace)
        mayShowOverFullScreen(window)
        // While FERB is the active app, pointer moves go to FERB itself, which the notch's global monitor never sees:
        // its local monitor sees them only through a key window that accepts them, so the notch still opens on hover
        // with this window in front (APP-656).
        window.acceptsMouseMovedEvents = true
        if window.isMiniaturized { window.deminiaturize(nil) }
        let inDock = policy() == .regular
        joinDock(for: window)
        // Left open on another Space: it comes here before anything orders it front (APP-774).
        let wasElsewhere = isShowing(window) && bringHere(window, "window")
        if !isShowing(window), let chat = avoid(), window.frame.intersects(chat),
           let screen = NSScreen.screens.first(where: { $0.frame.intersects(chat) }) ?? NSScreen.main,
           let beside = beside(window.frame, avoiding: chat, in: screen.visibleFrame, minSize: window.minSize) {
            window.setFrame(beside, display: false)
        }
        orderIn(window)
        bringHere(window, "window")
        makeKey(window)
        let activates = mayActivate(inDockBefore: inDock)
        if activates {
            activate()
            makeKey(window)
        }
        watch(newly: [window])
        log.notice("FERB window shows (was on another Space \(wasElsewhere, privacy: .public)); FERB was in the Dock \(inDock, privacy: .public), activates \(activates, privacy: .public)")
    }

    // MARK: - What AppKit or a library puts on screen for FERB (alerts, file pickers, Quick Look, the update window)

    /// Runs `open`; for `moment` seconds after, any FERB window that wasn't on screen before and lands only on another
    /// Space comes to the Space showing. FERB becomes the active app for it only when `mayActivate` allows.
    @discardableResult
    static func opening<T>(_ what: String, activating: Bool = false, for moment: TimeInterval = momentLength,
                           _ open: () -> T) -> T {
        start()
        let before = Set(visibleWindows().map(ObjectIdentifier.init))
        beginMoment(seen: before, until: now().addingTimeInterval(moment))
        if activating, mayActivate(inDockBefore: policy() == .regular) { activate() }
        log.notice("FERB opens \(what, privacy: .public)")
        return open()
    }

    /// An alert, on the Space showing.
    static func runModal(_ alert: NSAlert) -> NSApplication.ModalResponse {
        opening("an alert", activating: true) { runAlert(alert) }
    }

    /// A file picker run as its own modal window, on the Space showing.
    static func runModal(_ panel: NSSavePanel) -> NSApplication.ModalResponse {
        opening("a file picker", activating: true) { runPanel(panel) }
    }

    // MARK: - FERB's menus (the menu bar icon's, the profile menu, pickers, Menus, context menus)

    /// The status item's menu is about to open (its delegate's menuWillOpen): its window is kept on the Space showing.
    static func menuOpening() {
        start()
        beginMoment(seen: nil, until: now().addingTimeInterval(momentLength))
    }

    /// FERB's menus on screen now: its windows at the pop-up menu level or above that aren't its own panels (tests
    /// stand in their own).
    static var menuWindows: () -> [NSWindow] = {
        NSApp.windows.filter { $0.isVisible && $0.level >= .popUpMenu && $0.level < .screenSaver }
    }

    // MARK: - The moment's watch

    /// How long after FERB shows something, or a press in one of its windows, the watch keeps what opens here.
    nonisolated static let momentLength: TimeInterval = 1
    private static var openMenus: Set<ObjectIdentifier> = []
    private static var watchUntil: Date?
    /// Windows on screen when the moment began: only those that came after are FERB's new windows (a window the owner
    /// left open on another Space stays where it is).
    private static var seen: Set<ObjectIdentifier> = []
    private static var watched = NSHashTable<NSWindow>.weakObjects()
    private static var timer: Timer?
    private static var observers: [NSObjectProtocol] = []
    private static var pressMonitor: Any?

    /// Whether the watch runs now.
    static var watching: Bool { timer != nil }

    /// The menu and press observers, once.
    static func start() {
        guard observers.isEmpty else { return }
        let center = NotificationCenter.default
        observers.append(center.addObserver(forName: NSMenu.didBeginTrackingNotification, object: nil,
                                            queue: nil) { note in
            guard let menu = note.object as AnyObject? else { return }
            let id = ObjectIdentifier(menu)
            MainActor.assumeIsolated { menuBegan(id) }
        })
        observers.append(center.addObserver(forName: NSMenu.didEndTrackingNotification, object: nil,
                                            queue: nil) { note in
            guard let menu = note.object as AnyObject? else { return }
            let id = ObjectIdentifier(menu)
            MainActor.assumeIsolated { menuEnded(id) }
        })
        // A picker opens its menu on the press, sometimes without menu tracking
        pressMonitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { event in
            MainActor.assumeIsolated { if event.window != nil { pressed() } }
            return event
        }
    }

    static func menuBegan(_ menu: ObjectIdentifier) {
        openMenus.insert(menu)
        beginMoment(seen: nil, until: watchUntil ?? now())
    }

    static func menuEnded(_ menu: ObjectIdentifier) {
        openMenus.remove(menu)
        tick()
    }

    /// A press in one of FERB's windows.
    static func pressed() {
        beginMoment(seen: nil, until: now().addingTimeInterval(momentLength))
    }

    /// Starts or extends the moment. `seen` is the windows on screen before something new opens (nil: what shows now);
    /// a moment already running keeps the windows it had seen.
    private static func beginMoment(seen windows: Set<ObjectIdentifier>?, until: Date) {
        if timer == nil { seen = windows ?? Set(visibleWindows().map(ObjectIdentifier.init)) }
        watchUntil = max(watchUntil ?? until, until)
        tick()
        guard timer == nil, watchUntil.map({ now() < $0 }) == true || !openMenus.isEmpty else { return }
        // In the common modes, so it also runs while a menu tracks the pointer or an alert runs modally.
        let next = Timer(timeInterval: 0.05, repeats: true) { _ in MainActor.assumeIsolated { tick() } }
        RunLoop.main.add(next, forMode: .common)
        timer = next
    }

    /// FERB's windows shown just now, kept on the Space showing for the moment.
    private static func watch(newly windows: [NSWindow]) {
        beginMoment(seen: nil, until: now().addingTimeInterval(momentLength))
        for window in windows { watched.add(window); seen.remove(ObjectIdentifier(window)) }
    }

    /// One look: every FERB menu, and every FERB window new since the moment began, that is elsewhere comes here. The
    /// watch ends once no menu is open and the moment is over.
    static func tick() {
        guard watchUntil != nil || !openMenus.isEmpty else { return }
        for menu in menuWindows() { bringHere(menu, "menu") }
        for window in visibleWindows() where !seen.contains(ObjectIdentifier(window)) && window.level < .popUpMenu {
            bringHere(window, "window")
        }
        for window in watched.allObjects where isShowing(window) { bringHere(window, "window") }
        guard openMenus.isEmpty, watchUntil.map({ now() >= $0 }) ?? true else { return }
        watchUntil = nil
        watched.removeAllObjects()
        seen = []
        timer?.invalidate()
        timer = nil
    }

    /// A frame for `frame` beside `chat` (left or right, whichever side has more room) inside `visible`, as wide and
    /// tall as it was where that fits and never under `minSize`; nil when neither side has room (it then opens where
    /// it was and the sticky window steps back for it, APP-749).
    static func beside(_ frame: CGRect, avoiding chat: CGRect, in visible: CGRect, minSize: CGSize,
                       gap: CGFloat = 12) -> CGRect? {
        let right = visible.maxX - chat.maxX - gap, left = chat.minX - visible.minX - gap
        let room = max(right, left)
        guard room >= minSize.width, visible.height >= minSize.height else { return nil }
        let width = min(frame.width, room), height = min(frame.height, visible.height)
        let x = right >= left ? chat.maxX + gap : chat.minX - gap - width
        let y = min(max(frame.minY, visible.minY), visible.maxY - height)
        return CGRect(x: x, y: y, width: width, height: height)
    }

    static let log = Logger(subsystem: "com.ferb.FERB", category: "spaces")

    /// Like the notch's optional SkyLight bridge, this operates only on a window owned by this process.
    /// It moves that window, never the owner's current Space, and leaves the ordinary window level intact.
    private static func moveToActiveSpace(_ window: NSWindow) {
        typealias MainConnection = @convention(c) () -> Int32
        typealias ActiveSpace = @convention(c) (Int32) -> UInt64
        typealias MoveWindows = @convention(c) (Int32, UInt64, CFArray, Int32) -> Int32
        guard let library = dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/Versions/A/SkyLight", RTLD_NOW) else { return }
        defer { dlclose(library) }
        guard let main = dlsym(library, "SLSMainConnectionID"),
              let active = dlsym(library, "SLSGetActiveSpace"),
              let move = dlsym(library, "SLSSpaceAddWindowsAndRemoveFromSpaces") else { return }
        let connection = unsafeBitCast(main, to: MainConnection.self)()
        let space = unsafeBitCast(active, to: ActiveSpace.self)(connection)
        guard space != 0 else { return }
        _ = unsafeBitCast(move, to: MoveWindows.self)(connection, space, [NSNumber(value: window.windowNumber)] as CFArray, 7)
    }
}

// MARK: - Every way FERB shows a window or a menu

extension AppWindowPresentation {
    /// How a path meets the rule.
    enum Mechanism: String {
        /// `show`: FERB's own titled window; joins the Dock first, comes here (over a full-screen app too), activates
        /// only when that can't switch.
        case window
        /// `opening`/`runModal`: AppKit or a library puts it on screen; new windows that land elsewhere come here.
        case opening
        /// The moment's watch over FERB's menus (menu tracking, a press, the status item's menuWillOpen).
        case menu
        /// On every Space by construction: FERB's fixed overlay space or the all-Spaces collection behavior (APP-563,
        /// APP-760); never moved, never activates FERB.
        case everySpace
        /// A sheet, alert or popover attached to one of FERB's windows: it shows where its window is.
        case withParent
    }

    /// One way FERB shows a window or menu: what the owner sees, how the rule covers it, and the source file and code
    /// that open it (the structural test matches these against the sources).
    struct Path: Hashable {
        let name: String
        let mechanism: Mechanism
        let file: String
        let code: String
    }

    /// Every path. The debug-only playground (Sources/FERBNotch/Playground) isn't in FERB.app and isn't listed; its app
    /// window review uses `show` too.
    static let paths: [Path] = [
        // FERB's window and its pages: Settings… from the menu bar icon, ⌘, (FERB's menu), the profile menu's Settings…
        // and Open app window, Open FERB Window, a memory notice's page (FERBWindowController.show)
        .init(name: "FERB's window (Settings and every page)", mechanism: .window, file: "SecureEntryFocus.swift",
              code: "class FERBAppWindow: NSWindow"),
        .init(name: "FERB's window (Settings and every page)", mechanism: .window, file: "FERBWindow.swift",
              code: "FERBAppWindow(contentRect"),
        // Alerts
        .init(name: "Set Up From This App? (setup)", mechanism: .opening, file: "AppSetupController.swift", code: "NSAlert()"),
        .init(name: "Quit and Install? (update)", mechanism: .opening, file: "UpdateCoordinator.swift", code: "NSAlert()"),
        .init(name: "The artifact couldn't be opened; the service isn't installed", mechanism: .opening,
              file: "PanelCoordinator.swift", code: "NSAlert()"),
        // File pickers
        .init(name: "The chat's file picker (notch and sticky window)", mechanism: .opening, file: "NotchModel.swift",
              code: "NSOpenPanel()"),
        .init(name: "Memory → Import from… file", mechanism: .opening, file: "WindowPages.swift", code: "NSOpenPanel()"),
        // Quick Look and the update window
        .init(name: "Quick Look of an artifact", mechanism: .opening, file: "PanelCoordinator.swift",
              code: "QLPreviewPanel.shared()"),
        .init(name: "Sparkle's update window (Install Update…)", mechanism: .opening, file: "SparkleUpdater.swift",
              code: "checkForUpdates(nil)"),
        // Menus
        .init(name: "The menu bar icon's menu", mechanism: .menu, file: "PanelCoordinator.swift",
              code: "statusItem(withLength"),
        .init(name: "The profile menu (notch, sticky window)", mechanism: .menu, file: "ProfileMenuButton.swift",
              code: ".popUp(positioning"),
        .init(name: "A message's context menu", mechanism: .menu, file: "ArtifactPreview.swift", code: "func menu(for event"),
        .init(name: "A file card's context menu", mechanism: .menu, file: "ArtifactPreview.swift", code: ".contextMenu"),
        .init(name: "Settings' pickers", mechanism: .menu, file: "SettingsPages.swift", code: "Picker("),
        .init(name: "Settings → AI and setup pickers", mechanism: .menu, file: "SetupView.swift", code: "Picker("),
        .init(name: "Memory → Import from…", mechanism: .menu, file: "WindowPages.swift", code: "Menu("),
        // On every Space by construction
        .init(name: "The notch", mechanism: .everySpace, file: "PanelCoordinator.swift", code: "class NotchPanel: NSPanel"),
        .init(name: "The notch's light (trace)", mechanism: .everySpace, file: "PanelCoordinator.swift",
              code: "class NotchLightPanel: NSPanel"),
        .init(name: "The edge pill and its top row", mechanism: .everySpace, file: "FerbIndicator.swift",
              code: "class FerbIndicatorPanel: NSPanel"),
        .init(name: "The sticky window", mechanism: .everySpace, file: "ChatWindow.swift",
              code: "class ChatWindowPanel: NSPanel"),
        .init(name: "The task activity window", mechanism: .everySpace, file: "TaskActivityBubble.swift",
              code: "class TaskActivityPanel: NSPanel"),
        .init(name: "The intro's orb", mechanism: .everySpace, file: "OrbPanel.swift", code: "class OrbPanel: NSPanel"),
        .init(name: "The intro's drag card", mechanism: .everySpace, file: "OrbPanel.swift", code: "class OrbCardPanel: NSPanel"),
        .init(name: "The intro's drag demonstration", mechanism: .everySpace, file: "OrbPanel.swift",
              code: "class DragDemoPanel: NSPanel"),
        .init(name: "FERB's pointer overlay", mechanism: .everySpace, file: "CursorOverlay.swift",
              code: "class CursorPanel: NSPanel"),
        .init(name: "The first run's arrival", mechanism: .everySpace, file: "StartupArrival.swift",
              code: "class ArrivalPanel: NSPanel"),
        .init(name: "A reply's file list (a popover on the notch or the sticky window)", mechanism: .withParent,
              file: "ArtifactPreview.swift", code: ".popover("),
        // With their window
        .init(name: "The task activity window's popovers", mechanism: .withParent, file: "TaskActivityBubble.swift",
              code: ".popover("),
        .init(name: "An enlarged picture: a payment page, a card row's or a task's live preview (APP-889, APP-890)",
              mechanism: .withParent, file: "VisualComponents.swift", code: ".popover("),
        .init(name: "Settings' confirmations (Uninstall, Delete secret)", mechanism: .withParent,
              file: "SettingsPages.swift", code: ".alert("),
        .init(name: "Memory's Forget confirmation", mechanism: .withParent, file: "WindowPages.swift", code: ".alert("),
    ]
    // What counts as a presenter in the sources is listed by the build's scan (Tools/SettingsKeyScan, SourcePresenters).
}
