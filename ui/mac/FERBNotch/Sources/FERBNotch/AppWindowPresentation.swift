import AppKit
import Darwin

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

    static func show(_ window: NSWindow) {
        // Ordering it in or making it key brings the window to the owner's Space instead of switching Spaces.
        window.collectionBehavior.insert(.moveToActiveSpace)
        // While FERB is the active app, pointer moves go to FERB itself, which the notch's global monitor never sees:
        // its local monitor sees them only through a key window that accepts them, so the notch still opens on hover
        // with this window in front (APP-656).
        window.acceptsMouseMovedEvents = true
        if window.isMiniaturized { window.deminiaturize(nil) }
        window.orderFrontRegardless()
        if !window.isOnActiveSpace, !window.styleMask.contains(.fullScreen) { moveToActiveSpace(window) }
        // With its key window on this Space, activating FERB has no other Space to switch to.
        if window.canBecomeKey { window.makeKey() }
        activate()
        window.makeKeyAndOrderFront(nil)
    }

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
