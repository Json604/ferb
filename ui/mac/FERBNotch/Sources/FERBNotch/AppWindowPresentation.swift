import AppKit
import Darwin
import os

/// A requested app window must appear where the owner is working, including over a full-screen app.
/// AppKit can mark a regular window visible without assigning it a Space in that case (macOS 26).
///
/// The window is on the owner's Space before FERB activates (APP-688). Activating first let macOS switch to a Space
/// that still held the window (owner, 2026-11-04: Settings switched the screen to the home Space) while the window was
/// then moved to the Space the owner had been on, because the move read the active Space before the switch landed.
///
/// It is also FERB's key window before FERB activates (APP-773). The window the owner clicked to ask for it (the sticky
/// window's or the notch's profile menu) was key, and both live in FERB's overlay space, on no ordinary Space: activating
/// with one of them key, macOS switched to the home Space while the requested window stayed on the Space the owner
/// had been on (owner, 2026-11-06, window mode).
@MainActor
enum AppWindowPresentation {
    /// FERB's activation (tests record it instead).
    static var activate: () -> Void = { NSApp.activate(ignoringOtherApps: true) }

    /// The sticky window's frame while window mode shows it (PanelCoordinator sets it): a window brought on screen
    /// opens beside it rather than over or under it, where the screen has room (APP-774).
    static var avoid: () -> CGRect? = { nil }

    static func show(_ window: NSWindow) {
        // Ordering it in or making it key brings the window to the owner's Space instead of switching Spaces.
        window.collectionBehavior.insert(.moveToActiveSpace)
        // While FERB is the active app, pointer moves go to FERB itself, which the notch's global monitor never sees:
        // its local monitor sees them only through a key window that accepts them, so the notch still opens on hover
        // with this window in front (APP-656).
        window.acceptsMouseMovedEvents = true
        if window.isMiniaturized { window.deminiaturize(nil) }
        // Still showing on another Space (left open there): ordering a shown window front or activating FERB switched
        // the owner to that Space (owner, 2026-11-06, both modes). Taken off the screen first, it comes back on the
        // Space showing, as a window ordered in with moveToActiveSpace does (APP-774).
        let elsewhere = window.isVisible && !window.isOnActiveSpace && !window.styleMask.contains(.fullScreen)
        if elsewhere { window.orderOut(nil) }
        if !window.isVisible, let chat = avoid(), window.frame.intersects(chat),
           let screen = NSScreen.screens.first(where: { $0.frame.intersects(chat) }) ?? NSScreen.main,
           let beside = beside(window.frame, avoiding: chat, in: screen.visibleFrame, minSize: window.minSize) {
            window.setFrame(beside, display: false)
        }
        window.orderFrontRegardless()
        log.notice("FERB window shows (was on another Space \(elsewhere, privacy: .public)); on this Space \(window.isOnActiveSpace, privacy: .public)")
        if !window.isOnActiveSpace, !window.styleMask.contains(.fullScreen) { moveToActiveSpace(window) }
        // With its key window on this Space, activating FERB has no other Space to switch to.
        if window.canBecomeKey { window.makeKey() }
        activate()
        window.makeKeyAndOrderFront(nil)
        follow(window)
    }

    // MARK: - Following a Space switch FERB's activation makes (APP-775)

    /// How long after FERB's window shows a Space switch still counts as the one its activation made.
    static let followWindow: TimeInterval = 1.5
    /// Space changes (tests post their own), the time, whether a window is on the Space showing, and the move.
    static var spaces: NotificationCenter = NSWorkspace.shared.notificationCenter
    static var now: () -> Date = { Date() }
    static var isHere: (NSWindow) -> Bool = { $0.isOnActiveSpace }
    static var bringHere: (NSWindow) -> Void = { window in
        moveToActiveSpace(window)                        // the switch has landed: the Space showing is the new one
        if !window.isOnActiveSpace { window.orderOut(nil); window.orderFrontRegardless() }
        window.makeKeyAndOrderFront(nil)
    }
    private static var following: NSObjectProtocol?

    /// macOS may still switch the owner to another Space as FERB comes forward, with the window already shown on the
    /// Space they were on (live 2026-11-06: shown on 1380, then 1380 → 3 within 0.4 s). The owner accepts the switch;
    /// the window goes where it lands (owner: "the settings window shuold open on that home space but it opens on
    /// currnet space but still shifts"). Only the first Space change within `followWindow` counts: the owner's own
    /// switch later leaves the window where it is.
    private static func follow(_ window: NSWindow) {
        if let following { spaces.removeObserver(following) }
        let shown = now()
        following = spaces.addObserver(forName: NSWorkspace.activeSpaceDidChangeNotification, object: nil,
                                       queue: .main) { [weak window] _ in
            MainActor.assumeIsolated {
                if let following { spaces.removeObserver(following) }
                following = nil
                guard let window, window.isVisible, now().timeIntervalSince(shown) < followWindow, !isHere(window)
                else { return }
                log.notice("FERB window follows FERB's Space switch")
                bringHere(window)
            }
        }
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
