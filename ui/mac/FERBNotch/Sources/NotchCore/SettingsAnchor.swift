import Foundation
import CoreGraphics

/// One window as the window server reports it, in Quartz coordinates (origin at the top left of the primary display).
public struct RawWindow: Sendable, Equatable {
    public var pid: Int32
    public var bounds: CGRect
    public var layer: Int
    public init(pid: Int32, bounds: CGRect, layer: Int) {
        self.pid = pid
        self.bounds = bounds
        self.layer = layer
    }
}

/// A window of some app on screen, in AppKit global coordinates (origin at the bottom left of the primary display,
/// the space NSWindow frames use). `owner` is the app's bundle id, never its localised name.
public struct WindowInfo: Sendable, Equatable {
    public var owner: String
    public var bounds: CGRect
    public var onScreen: Bool
    public var isFrontmostApp: Bool
    public init(owner: String, bounds: CGRect, onScreen: Bool, isFrontmostApp: Bool) {
        self.owner = owner
        self.bounds = bounds
        self.onScreen = onScreen
        self.isFrontmostApp = isFrontmostApp
    }
}

/// Where the orb docks beside System Settings while it holds the draggable FERB icon.
public enum SettingsAnchor {
    public static let bundleID = "com.apple.systempreferences"
    /// Distance between the window's edge and the orb's centre.
    public static let gap: CGFloat = 16

    /// Quartz (top-left origin) to AppKit (bottom-left origin); `primaryHeight` is the height of the display whose
    /// AppKit frame origin is (0, 0). Displays above or left of the primary give negative or past-the-top values.
    public static func appKitRect(fromQuartz r: CGRect, primaryHeight: CGFloat) -> CGRect {
        CGRect(x: r.minX, y: primaryHeight - r.maxY, width: r.width, height: r.height)
    }

    /// Settings' on-screen window the orb follows: the largest one (sheets and tooltips are smaller).
    public static func window(in windows: [WindowInfo]) -> WindowInfo? { largestWindow(of: bundleID, in: windows) }

    /// An app's main on-screen window: its largest (sheets and tooltips are smaller).
    public static func largestWindow(of owner: String, in windows: [WindowInfo]) -> WindowInfo? {
        windows
            .filter { $0.owner == owner && $0.onScreen && $0.bounds.width > 0 && $0.bounds.height > 0 }
            .max { $0.bounds.width * $0.bounds.height < $1.bounds.width * $1.bounds.height }
    }

    /// The orb's centre beside Settings' window: to its left, else its right, clamped vertically into `screen`.
    /// Nil when Settings isn't in front, has no on-screen window, or the orb fits on neither side.
    public static func dock(for windows: [WindowInfo], orb: CGSize, screen: CGRect) -> CGPoint? {
        guard let w = window(in: windows), w.isFrontmostApp else { return nil }
        return beside(w, orb: orb, screen: screen)
    }

    /// The orb's centre beside a window: to its left, else its right, clamped vertically into `screen`; nil when the
    /// orb fits on neither side.
    public static func beside(_ w: WindowInfo, orb: CGSize, screen: CGRect) -> CGPoint? {
        let half = CGSize(width: orb.width / 2, height: orb.height / 2)
        let left = w.bounds.minX - gap
        let right = w.bounds.maxX + gap
        let x: CGFloat
        if left - half.width >= screen.minX {
            x = left
        } else if right + half.width <= screen.maxX {
            x = right
        } else {
            return nil
        }
        guard screen.height >= orb.height else { return nil }
        let y = min(max(w.bounds.midY, screen.minY + half.height), screen.maxY - half.height)
        return CGPoint(x: x, y: y)
    }

    /// True when both frames exist and every edge moved by at most 1 pt.
    public static func settled(_ a: CGRect?, _ b: CGRect?) -> Bool {
        guard let a, let b else { return false }
        return abs(a.minX - b.minX) <= 1 && abs(a.maxX - b.maxX) <= 1
            && abs(a.minY - b.minY) <= 1 && abs(a.maxY - b.maxY) <= 1
    }
}
