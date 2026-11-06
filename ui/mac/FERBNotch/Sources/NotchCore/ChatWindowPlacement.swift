import CoreGraphics
import Foundation

/// Where FERB's chat opens (D19, APP-745): in the notch (the default) or in the sticky window. Settings → Notch &
/// Shortcuts → Where FERB opens; stored under `AppSettings.whereFERBOpens`.
public enum ChatHost: String, Equatable, Sendable {
    case notch, window

    public static let defaultsKey = "chat.opensIn"

    public static func setting(_ defaults: UserDefaults) -> ChatHost {
        defaults.string(forKey: defaultsKey).flatMap(ChatHost.init(rawValue:)) ?? .notch
    }

    /// The window's one choice, read only in window mode (D21 Hide the pill). The window always floats (APP-772).
    public static let hidePillKey = "chatWindow.hidePill"
    /// Keep on top's old key (D20): the setting is gone (owner 2026-11-06, APP-772); a stored value is cleared on
    /// upgrade and never read.
    public static let retiredKeepOnTopKey = "chatWindow.keepOnTop"

    /// Clears what removed window settings left (APP-772, as APP-527 cleared its setting).
    public static func clearRetired(_ defaults: UserDefaults) {
        defaults.removeObject(forKey: retiredKeepOnTopKey)
    }

    /// Hide the pill: off unless the owner turned it on, and only ever in window mode (D21).
    public static func hidesPill(_ defaults: UserDefaults) -> Bool {
        setting(defaults) == .window && (defaults.object(forKey: hidePillKey) as? Bool ?? false)
    }
}

/// The sticky window's place on each display (APP-745): one frame per display id, in AppKit screen coordinates,
/// saved when the owner moves or resizes it and used when it opens on that display; a frame that no longer fits a
/// visible screen (a display unplugged, a resolution change) is moved onto one.
public enum ChatWindowFrames {
    public static let defaultsKey = "chatWindow.frames"
    /// The smallest window that still shows the composer and a few messages. A layout detail, not a cap.
    public static let minimumSize = CGSize(width: 360, height: 320)
    public static let standardSize = CGSize(width: 440, height: 600)
    /// The margin a new window keeps from the visible area's edges.
    static let margin: CGFloat = 24

    public static func saved(display: UInt32, in defaults: UserDefaults) -> CGRect? {
        guard let all = defaults.dictionary(forKey: defaultsKey), let numbers = all[String(display)] as? [Double],
              numbers.count == 4 else { return nil }
        return CGRect(x: numbers[0], y: numbers[1], width: numbers[2], height: numbers[3])
    }

    public static func save(_ frame: CGRect, display: UInt32, in defaults: UserDefaults) {
        var all = defaults.dictionary(forKey: defaultsKey) ?? [:]
        all[String(display)] = [frame.minX, frame.minY, frame.width, frame.height].map { Double($0) }
        defaults.set(all, forKey: defaultsKey)
    }

    /// How many displays have a saved place (the guide's value).
    public static func count(in defaults: UserDefaults) -> Int { defaults.dictionary(forKey: defaultsKey)?.count ?? 0 }

    /// A first window on a display: the standard size at the visible area's top right.
    public static func standard(in visible: CGRect) -> CGRect {
        fitted(CGRect(x: visible.maxX - margin - standardSize.width, y: visible.maxY - margin - standardSize.height,
                      width: standardSize.width, height: standardSize.height), in: visible)
    }

    /// The frame moved (and shrunk if it must) so all of it lies on the visible area, never under the minimum size.
    public static func fitted(_ frame: CGRect, in visible: CGRect) -> CGRect {
        let width = min(max(frame.width, minimumSize.width), max(visible.width, minimumSize.width))
        let height = min(max(frame.height, minimumSize.height), max(visible.height, minimumSize.height))
        let x = min(max(frame.minX, visible.minX), visible.maxX - width)
        let y = min(max(frame.minY, visible.minY), visible.maxY - height)
        return CGRect(x: x, y: y, width: width, height: height)
    }

    /// Where the window opens on a display: its saved place there fitted to the visible area, else the standard one.
    public static func frame(display: UInt32?, visible: CGRect, in defaults: UserDefaults) -> CGRect {
        guard let display, let saved = saved(display: display, in: defaults) else { return standard(in: visible) }
        return fitted(saved, in: visible)
    }

    /// Whether the owner can still reach the window: its top strip (where it is dragged) lies on a visible area by at
    /// least a grab's worth. False after its display went away.
    public static func isReachable(_ frame: CGRect, on visibles: [CGRect]) -> Bool {
        let grip = CGRect(x: frame.minX, y: frame.maxY - 28, width: frame.width, height: 28)
        return visibles.contains { visible in
            let shared = grip.intersection(visible)
            return !shared.isNull && shared.width >= 60 && shared.height >= 20
        }
    }
}

/// The menu bar icon's tint while the pill is hidden (D21): the pill's own status dot colours (APP-564), from the same
/// phase; none when FERB is idle, or while the pill shows its status itself.
public enum StatusTint: Equatable, Sendable {
    case amber, red, green

    public static func of(_ phase: SessionPhase, pillHidden: Bool) -> StatusTint? {
        guard pillHidden else { return nil }
        switch phase {
        case .working: return .amber
        case .attention: return .red
        case .result: return .green
        case .ready, .draft, .listening, .stopped: return nil
        }
    }
}
