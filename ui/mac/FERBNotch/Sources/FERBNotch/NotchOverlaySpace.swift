import AppKit
import Darwin
import NotchCore
import os

/// AppKit's all-Spaces windows still participate in the desktop swipe transform.
/// Put our notch windows in an unmanaged WindowServer space instead.
///
/// API sequence adapted from SkyLightWindow (MIT, Lakr Aream):
/// https://github.com/Lakr233/SkyLightWindow
/// See ThirdPartyNotices.txt. These private symbols are optional: if unavailable,
/// the coordinator retains the ordinary AppKit all-Spaces behavior.
///
/// Whether a window joined the space is read from the space's member list (and, for the space showing, the on-screen
/// list), never from the move and show calls' return codes: macOS 27.2 returns 3 from both even when they worked
/// (APP-704, OverlaySpaceCheck). The outcome goes to the system log (subsystem com.ferb.FERB, category notch).
@MainActor
final class NotchOverlaySpace {
    private static let log = Logger(subsystem: "com.ferb.FERB", category: "notch")
    private typealias Connection = @convention(c) () -> Int32
    private typealias Create = @convention(c) (Int32, UInt64, CFDictionary?) -> UInt64
    private typealias SetLevel = @convention(c) (Int32, UInt64, Int32) -> Int32
    private typealias Show = @convention(c) (Int32, CFArray) -> Int32
    private typealias MoveWindows = @convention(c) (Int32, UInt64, CFArray, Int32) -> Int32
    private typealias RemoveWindows = @convention(c) (Int32, CFArray, CFArray) -> Void
    private typealias Destroy = @convention(c) (Int32, UInt64) -> Void
    private typealias CopyWindows = @convention(c) (Int32, UInt32, CFArray, UInt32, UnsafeMutablePointer<UInt64>, UnsafeMutablePointer<UInt64>) -> Unmanaged<CFArray>?

    private var library: UnsafeMutableRawPointer?
    private var connection: Int32 = 0
    private var space: UInt64 = 0
    private var windows: [NSWindow] = []
    private var removeWindows: RemoveWindows?
    private var destroy: Destroy?
    private var moveWindows: MoveWindows?
    private var copyWindows: CopyWindows?

    var isAttached: Bool { space != 0 }

    func attach(_ windows: [NSWindow]) {
        guard !isAttached, !windows.isEmpty else { return }
        guard let library = dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/Versions/A/SkyLight", RTLD_NOW) else {
            Self.log.error("Fixed overlay unavailable (no SkyLight): the notch windows stay ordinary all-Spaces windows.")
            return
        }
        func symbol<T>(_ name: String, as type: T.Type) -> T? {
            guard let address = dlsym(library, name) else { return nil }
            return unsafeBitCast(address, to: type)
        }
        guard let mainConnection = symbol("SLSMainConnectionID", as: Connection.self),
              let create = symbol("SLSSpaceCreate", as: Create.self),
              let setLevel = symbol("SLSSpaceSetAbsoluteLevel", as: SetLevel.self),
              let show = symbol("SLSShowSpaces", as: Show.self),
              let move = symbol("SLSSpaceAddWindowsAndRemoveFromSpaces", as: MoveWindows.self),
              let remove = symbol("SLSRemoveWindowsFromSpaces", as: RemoveWindows.self),
              let destroy = symbol("SLSSpaceDestroy", as: Destroy.self),
              let copyWindows = symbol("SLSCopyWindowsWithOptionsAndTags", as: CopyWindows.self) else {
            dlclose(library)
            Self.log.error("Fixed overlay symbols missing: the notch windows stay ordinary all-Spaces windows.")
            return
        }

        let connection = mainConnection()
        // The unmanaged creation flag keeps this out of Mission Control's desktop list.
        let space = create(connection, 1, nil)
        guard space != 0 else {
            dlclose(library)
            Self.log.error("Fixed overlay space creation failed: the notch windows stay ordinary all-Spaces windows.")
            return
        }
        let numbers = windows.map(\.windowNumber)
        let windowIDs = numbers.map { NSNumber(value: $0) } as CFArray
        // Read from the same window-server list as afterwards (AppKit's isVisible can run ahead of the window server).
        let listedBefore = Self.onScreenWindowNumbers()
        let shownBefore = numbers.filter(listedBefore.contains)
        // Above the desktop stack (0), below Setup Assistant (100), security
        // prompts (200) and the lock screen (300). Never use the lock-screen level.
        let levelStatus = setLevel(connection, space, 1)
        // Nothing moves into a space whose level didn't take. The move's and show's own results are only logged:
        // macOS 27.2 returns 3 from both even when every window joined and the space shows (APP-704).
        var moveStatus: Int32 = -1
        var showStatus: Int32 = -1
        if levelStatus == 0 {
            moveStatus = move(connection, space, windowIDs, 7)
            showStatus = show(connection, [NSNumber(value: space)] as CFArray)
        }
        // Query the owned space directly: SLSCopySpacesForWindows only reports managed desktops here. Confirm every
        // window, including the hidden trace, and that the windows on screen before the move still are: a space that
        // doesn't show lists none of its windows on screen.
        let memberIDs = Self.members(of: space, connection: connection, copyWindows: copyWindows)
        let onScreen = Self.onScreenWindowNumbers()
        guard OverlaySpaceCheck.attached(windows: numbers, levelStatus: levelStatus, members: memberIDs,
                                         shownBefore: shownBefore, onScreen: onScreen) else {
            remove(connection, windowIDs, [NSNumber(value: space)] as CFArray)
            destroy(connection, space)
            for window in windows {
                let behavior = window.collectionBehavior
                window.collectionBehavior = []
                window.collectionBehavior = behavior
            }
            dlclose(library)
            let notShown = shownBefore.filter { !onScreen.contains($0) }
            let members = memberIDs.sorted()
            Self.log.error("""
                Fixed overlay attachment failed (level \(levelStatus, privacy: .public), move \(moveStatus, privacy: .public), \
                show \(showStatus, privacy: .public); windows \(numbers, privacy: .public), members \
                \(members, privacy: .public), shown before but not on screen \(notShown, privacy: .public)): the notch \
                windows stay ordinary all-Spaces windows.
                """)
            return
        }

        self.library = library
        self.connection = connection
        self.space = space
        self.windows = windows
        self.removeWindows = remove
        self.destroy = destroy
        self.moveWindows = move
        self.copyWindows = copyWindows
        Self.log.info("""
            Notch windows \(numbers, privacy: .public) attached to fixed overlay space \(space, privacy: .public) \
            (move \(moveStatus, privacy: .public), show \(showStatus, privacy: .public)).
            """)
    }

    /// The window numbers the space lists (one window-server read, no titles).
    private static func members(of space: UInt64, connection: Int32, copyWindows: CopyWindows) -> Set<Int> {
        var setTags: UInt64 = 0
        var clearTags: UInt64 = 0
        let members = copyWindows(connection, UInt32(bitPattern: connection),
                                  [NSNumber(value: space)] as CFArray, 7, &setTags, &clearTags)?.takeRetainedValue()
        return Set((members as? [NSNumber] ?? []).map(\.intValue))
    }

    /// The window numbers on screen now (numbers only: no titles, so no Screen Recording permission is involved).
    private static func onScreenWindowNumbers() -> Set<Int> {
        let rows = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
        return Set(rows.compactMap { ($0[kCGWindowNumber as String] as? NSNumber)?.intValue })
    }

    /// Moves one window into the space; whether it joined is what the space lists, never the move's result.
    private func moveIn(_ window: NSWindow) -> Bool {
        guard space != 0, let moveWindows, let copyWindows else { return false }
        let status = moveWindows(connection, space, [NSNumber(value: window.windowNumber)] as CFArray, 7)
        let joined = Self.members(of: space, connection: connection, copyWindows: copyWindows)
            .contains(window.windowNumber)
        if !joined {
            Self.log.error("""
                \(window.title, privacy: .public) could not join the fixed overlay space (move \(status, privacy: .public)).
                """)
        }
        return joined
    }

    /// Ordering one of our windows out and back in (the trace after each glow) may drop it from the space, and the
    /// panel leaves it while FERB yields (`leave`). Move it back in every time, with the same call `attach` uses.
    func rejoin(_ window: NSWindow) {
        guard space != 0, windows.contains(where: { $0 === window }) else { return }
        _ = moveIn(window)
    }

    /// Add an independently detached UI panel to FERB's fixed overlay when that space is available.
    func include(_ window: NSWindow) {
        guard space != 0, !windows.contains(where: { $0 === window }), moveIn(window) else { return }
        windows.append(window)
    }

    /// Return a detached panel to AppKit's ordinary all-Spaces behavior before releasing it.
    func remove(_ window: NSWindow) {
        guard space != 0, let removeWindows, windows.contains(where: { $0 === window }) else { return }
        removeWindows(connection, [NSNumber(value: window.windowNumber)] as CFArray, [NSNumber(value: space)] as CFArray)
        windows.removeAll { $0 === window }
        let behavior = window.collectionBehavior
        window.collectionBehavior = []
        window.collectionBehavior = behavior
    }

    /// FERB yields: the panel leaves the space for the ordinary window stack, so another app's window or menu can
    /// be in front of it while it stays on screen. Resetting its collection behavior has AppKit place it on the
    /// ordinary Spaces again, as `attach` does when attaching fails. `rejoin` brings it back.
    func leave(_ window: NSWindow) {
        guard space != 0, let removeWindows, windows.contains(where: { $0 === window }) else { return }
        removeWindows(connection, [NSNumber(value: window.windowNumber)] as CFArray, [NSNumber(value: space)] as CFArray)
        let behavior = window.collectionBehavior
        window.collectionBehavior = []
        window.collectionBehavior = behavior
    }

    func stop() {
        guard space != 0 else { return }
        // Remove only our windows and destroy only the space we created.
        removeWindows?(connection, windows.map { NSNumber(value: $0.windowNumber) } as CFArray,
                       [NSNumber(value: space)] as CFArray)
        destroy?(connection, space)
        space = 0
        windows.removeAll()
        removeWindows = nil
        destroy = nil
        moveWindows = nil
        copyWindows = nil
        if let library { dlclose(library) }
        library = nil
    }
}
