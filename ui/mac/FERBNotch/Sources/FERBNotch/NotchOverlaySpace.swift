import AppKit
import Darwin

/// AppKit's all-Spaces windows still participate in the desktop swipe transform.
/// Put our notch windows in an unmanaged WindowServer space instead.
///
/// API sequence adapted from SkyLightWindow (MIT, Lakr Aream):
/// https://github.com/Lakr233/SkyLightWindow
/// See ThirdPartyNotices.txt. These private symbols are optional: if unavailable,
/// the coordinator retains the ordinary AppKit all-Spaces behavior.
@MainActor
final class NotchOverlaySpace {
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

    var isAttached: Bool { space != 0 }

    func attach(_ windows: [NSWindow]) {
        guard !isAttached, !windows.isEmpty else { return }
        guard let library = dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/Versions/A/SkyLight", RTLD_NOW) else {
            NSLog("FERB Notch: fixed overlay unavailable; using AppKit all-Spaces windows.")
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
            NSLog("FERB Notch: fixed overlay symbols missing; using AppKit all-Spaces windows.")
            return
        }

        let connection = mainConnection()
        // The unmanaged creation flag keeps this out of Mission Control's desktop list.
        let space = create(connection, 1, nil)
        guard space != 0 else {
            dlclose(library)
            NSLog("FERB Notch: fixed overlay creation failed; using AppKit all-Spaces windows.")
            return
        }
        let windowIDs = windows.map { NSNumber(value: $0.windowNumber) } as CFArray
        // Above the desktop stack (0), below Setup Assistant (100), security
        // prompts (200) and the lock screen (300). Never use the lock-screen level.
        let levelStatus = setLevel(connection, space, 1)
        let moveStatus = levelStatus == 0 ? move(connection, space, windowIDs, 7) : levelStatus
        let showStatus = moveStatus == 0 ? show(connection, [NSNumber(value: space)] as CFArray) : moveStatus
        // Query the owned space directly: SLSCopySpacesForWindows only reports
        // managed desktops here. Confirm BOTH windows, including the hidden trace,
        // rather than treating symbol availability or a zero result as success.
        var setTags: UInt64 = 0
        var clearTags: UInt64 = 0
        let members = copyWindows(connection, UInt32(bitPattern: connection),
                                  [NSNumber(value: space)] as CFArray, 7, &setTags, &clearTags)?.takeRetainedValue()
        let memberIDs = (members as? [NSNumber] ?? []).map(\.intValue)
        let bothAttached = showStatus == 0 && windows.allSatisfy { memberIDs.contains($0.windowNumber) }
        guard bothAttached else {
            remove(connection, windowIDs, [NSNumber(value: space)] as CFArray)
            destroy(connection, space)
            for window in windows {
                let behavior = window.collectionBehavior
                window.collectionBehavior = []
                window.collectionBehavior = behavior
            }
            dlclose(library)
            NSLog("FERB Notch: fixed overlay attachment failed (%d/%d/%d, members %@); restored AppKit all-Spaces windows.", levelStatus, moveStatus, showStatus, memberIDs.description)
            return
        }

        self.library = library
        self.connection = connection
        self.space = space
        self.windows = windows
        self.removeWindows = remove
        self.destroy = destroy
        self.moveWindows = move
        NSLog("FERB Notch: dialog and trace attached to fixed overlay space %llu.", space)
    }

    /// Ordering one of our windows out and back in (the trace after each glow) may drop it from the space, and the
    /// panel leaves it while FERB yields (`leave`). Move it back in every time, with the same call `attach` uses.
    func rejoin(_ window: NSWindow) {
        guard space != 0, let moveWindows, windows.contains(where: { $0 === window }) else { return }
        let status = moveWindows(connection, space, [NSNumber(value: window.windowNumber)] as CFArray, 7)
        if status != 0 { NSLog("FERB Notch: %@ could not rejoin the fixed overlay space (%d).", window.title, status) }
    }

    /// Add an independently detached UI panel to FERB's fixed overlay when that space is available.
    func include(_ window: NSWindow) {
        guard space != 0, let moveWindows, !windows.contains(where: { $0 === window }) else { return }
        let status = moveWindows(connection, space, [NSNumber(value: window.windowNumber)] as CFArray, 7)
        guard status == 0 else {
            NSLog("FERB Notch: %@ could not join the fixed overlay space (%d).", window.title, status)
            return
        }
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
        if let library { dlclose(library) }
        library = nil
    }
}
