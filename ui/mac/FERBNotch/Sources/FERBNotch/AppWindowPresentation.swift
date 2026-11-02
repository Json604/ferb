import AppKit
import Darwin

/// A requested app window must appear where the owner is working, including over a full-screen app.
/// AppKit can mark a regular window visible without assigning it a Space in that case (macOS 26).
@MainActor
enum AppWindowPresentation {
    static func show(_ window: NSWindow) {
        if window.isMiniaturized { window.deminiaturize(nil) }
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        guard !window.isOnActiveSpace, !window.styleMask.contains(.fullScreen) else { return }
        moveToActiveSpace(window)
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
