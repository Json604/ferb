import CoreGraphics
import Foundation

/// Why FERB's notch windows step aside for another app. They live in an overlay space above every ordinary
/// window and above other apps' menus, so no window level lets another app's surface win: FERB has to get out
/// of the way. While any reason holds, the windows are invisible and click-through.
public enum NotchYieldReason: Hashable, Sendable, CaseIterable {
    /// FERB's own file picker is open.
    case filePicker
    /// Another process's menu, popover or floating window overlaps where FERB is drawn.
    case foreignOverlay
    /// The owner is working in another app and one of its windows lies under the pinned chat.
    case activeAppUnderPinnedChat
}

/// The set of reasons FERB is currently yielding. Visibility changes only when the set goes from empty to
/// non-empty or back, so one reason ending never brings FERB back while another still holds.
public struct NotchYield: Equatable, Sendable {
    public private(set) var reasons: Set<NotchYieldReason> = []

    public init() {}

    public var isActive: Bool { !reasons.isEmpty }

    /// Adds a reason and returns whether FERB's visibility changed. While the owner is dictating, another app's
    /// windows never hide FERB (its own file picker still does).
    @discardableResult
    public mutating func begin(_ reason: NotchYieldReason, listening: Bool = false) -> Bool {
        if listening && reason != .filePicker { return false }
        let wasActive = isActive
        reasons.insert(reason)
        return wasActive != isActive
    }

    /// Removes a reason and returns whether FERB's visibility changed.
    @discardableResult
    public mutating func end(_ reason: NotchYieldReason) -> Bool {
        let wasActive = isActive
        reasons.remove(reason)
        return wasActive != isActive
    }

    /// Dictation started: drop the reasons that come from other apps' windows. Returns whether visibility changed.
    @discardableResult
    public mutating func endWindowReasons() -> Bool {
        let wasActive = isActive
        reasons.remove(.foreignOverlay)
        reasons.remove(.activeAppUnderPinnedChat)
        return wasActive != isActive
    }
}

/// One on-screen window as the window server lists it. FERB reads only these four facts. It never reads a
/// window's name or title, so no Screen Recording permission is involved.
public struct ScreenWindow: Equatable, Sendable {
    public var ownerPID: Int32
    public var layer: Int
    /// Window-server coordinates: origin at the top left of the primary display, y growing downwards.
    public var bounds: CGRect
    public var alpha: Double

    public init(ownerPID: Int32, layer: Int, bounds: CGRect, alpha: Double) {
        self.ownerPID = ownerPID
        self.layer = layer
        self.bounds = bounds
        self.alpha = alpha
    }

    /// Reads one `CGWindowListCopyWindowInfo` row; nil when one of the four facts is missing.
    public init?(windowInfo row: [String: Any]) {
        guard let pid = (row[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value,
              let layer = (row[kCGWindowLayer as String] as? NSNumber)?.intValue,
              let bounds = row[kCGWindowBounds as String] as? [String: Any],
              let x = (bounds["X"] as? NSNumber)?.doubleValue, let y = (bounds["Y"] as? NSNumber)?.doubleValue,
              let width = (bounds["Width"] as? NSNumber)?.doubleValue,
              let height = (bounds["Height"] as? NSNumber)?.doubleValue else { return nil }
        let alpha = (row[kCGWindowAlpha as String] as? NSNumber)?.doubleValue ?? 1
        self.init(ownerPID: pid, layer: layer, bounds: CGRect(x: x, y: y, width: width, height: height), alpha: alpha)
    }

    /// The windows currently on screen, front to back, without the desktop picture and icons.
    public static func onScreen() -> [ScreenWindow] {
        let rows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
            as? [[String: Any]] ?? []
        return rows.compactMap(ScreenWindow.init(windowInfo:))
    }
}

/// Decisions about other apps' windows and FERB's frames. All rects are window-server coordinates.
public enum NotchOverlap {
    /// The Dock's own layer. The Dock is always on screen and is never one of the transient surfaces FERB yields to.
    public static let dockLayer = Int(CGWindowLevelForKey(.dockWindow))

    /// Whether another process has a menu, popover or floating window over one of FERB's frames.
    ///
    /// Counted: windows above the normal layer (layer > 0) that are visible (alpha > 0) and overlap a FERB frame.
    /// Never counted, so FERB doesn't hide for good: FERB's own processes; anything that stays within the menu
    /// bar strip (the menu bar itself, status items, the privacy indicator); the Dock layer; and windows that cover
    /// the whole display below the menu bar (screen dimmers, the screenshot crosshair, Launchpad), which are
    /// overlays of the entire screen rather than something FERB sits on top of.
    public static func foreignOverlay(in windows: [ScreenWindow], ferbFrames: [CGRect], ownPIDs: Set<Int32>,
                                      menuBar: CGRect, display: CGRect) -> Bool {
        let belowMenuBar = CGRect(x: display.minX, y: menuBar.maxY, width: display.width,
                                  height: max(0, display.maxY - menuBar.maxY))
        return windows.contains { window in
            guard !ownPIDs.contains(window.ownerPID), window.layer > 0, window.layer != dockLayer,
                  window.alpha > 0, !window.bounds.isEmpty else { return false }
            if menuBar.insetBy(dx: -1, dy: -1).contains(window.bounds) { return false }
            if !belowMenuBar.isEmpty, window.bounds.insetBy(dx: -1, dy: -1).contains(belowMenuBar) { return false }
            return ferbFrames.contains { overlaps(window.bounds, $0) }
        }
    }

    /// Whether the app has a visible window on the normal layer under the pinned chat's visible surface.
    public static func appWindowUnderChat(in windows: [ScreenWindow], appPID: Int32, chat: CGRect) -> Bool {
        windows.contains { window in
            window.ownerPID == appPID && window.layer == 0 && window.alpha > 0 && overlaps(window.bounds, chat)
        }
    }

    /// The process that owns the frontmost visible window at a point, skipping FERB's own windows.
    public static func owner(at point: CGPoint, in windows: [ScreenWindow], ownPIDs: Set<Int32>) -> Int32? {
        windows.first { window in
            !ownPIDs.contains(window.ownerPID) && window.layer >= 0 && window.alpha > 0 && window.bounds.contains(point)
        }?.ownerPID
    }

    /// At least one point of shared area: windows that only touch along an edge don't overlap.
    public static func overlaps(_ first: CGRect, _ second: CGRect) -> Bool {
        let shared = first.intersection(second)
        return !shared.isNull && shared.width >= 1 && shared.height >= 1
    }

    /// Converts an AppKit screen rect (origin at the bottom left of the primary display) to window-server
    /// coordinates (origin at its top left).
    public static func windowServerRect(_ rect: CGRect, primaryDisplayHeight: CGFloat) -> CGRect {
        CGRect(x: rect.minX, y: primaryDisplayHeight - rect.maxY, width: rect.width, height: rect.height)
    }

    public static func windowServerPoint(_ point: CGPoint, primaryDisplayHeight: CGFloat) -> CGPoint {
        CGPoint(x: point.x, y: primaryDisplayHeight - point.y)
    }
}
