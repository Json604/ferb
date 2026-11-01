import AppKit
import NotchCore

/// Follows System Settings' window so the orb can dock beside it. It reads window bounds only (never names or
/// titles, which would need Screen Recording) and reports where the orb's centre belongs, or nil when it should hide.
@MainActor
final class SettingsWindowTracker {
    /// Called when the dock point moves by more than 1 pt or switches between nil and a point.
    var onChange: ((CGPoint?) -> Void)?

    private let orb: CGSize
    private let rawWindows: () -> [RawWindow]
    private let bundleID: (Int32) -> String?
    private let frontmostBundleID: () -> String?
    private let screens: () -> [(frame: CGRect, visible: CGRect)]
    private let primaryHeight: () -> CGFloat
    private var timer: Timer?
    private var reported: CGPoint?
    private var previousFrame: CGRect?
    private var opened = false

    init(
        orb: CGSize,
        rawWindows: @escaping () -> [RawWindow] = SettingsWindowTracker.systemWindows,
        bundleID: @escaping (Int32) -> String? = { NSRunningApplication(processIdentifier: $0)?.bundleIdentifier },
        frontmostBundleID: @escaping () -> String? = { NSWorkspace.shared.frontmostApplication?.bundleIdentifier },
        screens: @escaping () -> [(frame: CGRect, visible: CGRect)] = {
            NSScreen.screens.map { (frame: $0.frame, visible: $0.visibleFrame) }
        },
        primaryHeight: @escaping () -> CGFloat = {
            NSScreen.screens.first { $0.frame.origin == .zero }?.frame.height ?? 0
        }
    ) {
        self.orb = orb
        self.rawWindows = rawWindows
        self.bundleID = bundleID
        self.frontmostBundleID = frontmostBundleID
        self.screens = screens
        self.primaryHeight = primaryHeight
    }

    func start() {
        guard timer == nil else { return }
        let t = Timer(timeInterval: 0.25, repeats: true) { [weak self] timer in
            MainActor.assumeIsolated {
                if let self { self.poll() } else { timer.invalidate() }
            }
        }
        RunLoop.main.add(t, forMode: .common)
        timer = t
        poll()
    }

    /// Stops polling and forgets what was reported, so a later start() reports the same window again. It does not
    /// emit nil: the caller hides the orb itself when it stops tracking.
    func stop() {
        timer?.invalidate()
        timer = nil
        reported = nil
        previousFrame = nil
        opened = false
    }

    /// One look at the window server. The first point waits for two settled frames so the orb doesn't chase
    /// Settings' opening animation; after that moves are reported at once so the orb follows a drag.
    func poll() {
        let point = dockPoint()
        guard let (p, frame) = point else {
            previousFrame = nil
            opened = false
            if reported != nil {
                reported = nil
                onChange?(nil)
            }
            return
        }
        if !opened {
            let ready = SettingsAnchor.settled(previousFrame, frame)
            previousFrame = frame
            guard ready else { return }
            opened = true
        }
        previousFrame = frame
        if let r = reported, abs(r.x - p.x) <= 1, abs(r.y - p.y) <= 1 { return }
        reported = p
        onChange?(p)
    }

    private func dockPoint() -> (CGPoint, CGRect)? {
        let all = screens()
        let primary = primaryHeight()
        let frontmost = frontmostBundleID() == SettingsAnchor.bundleID
        var resolved: [Int32: Bool] = [:]  // each PID looked up once per poll
        let infos = SettingsAnchor.windows(rawWindows(), owner: SettingsAnchor.bundleID, primaryHeight: primary,
                                           frontmost: frontmost) { pid in
            let isSettings = resolved[pid] ?? (bundleID(pid) == SettingsAnchor.bundleID)
            resolved[pid] = isSettings
            return isSettings
        }
        guard let w = SettingsAnchor.window(in: infos) else { return nil }
        let mid = CGPoint(x: w.bounds.midX, y: w.bounds.midY)
        // Fallback: Settings off every screen (mid-drag between displays) docks on the nearest screen.
        guard let screen = SettingsAnchor.screen(holding: w, in: all)
                ?? all.min(by: { Self.distance(from: mid, to: $0.frame) < Self.distance(from: mid, to: $1.frame) })?.visible,
              let p = SettingsAnchor.dock(for: infos, orb: orb, screen: screen) else { return nil }
        return (p, w.bounds)
    }

    private static func distance(from p: CGPoint, to r: CGRect) -> CGFloat {
        let dx = max(r.minX - p.x, 0, p.x - r.maxX)
        let dy = max(r.minY - p.y, 0, p.y - r.maxY)
        return (dx * dx + dy * dy).squareRoot()
    }

    nonisolated static func systemWindows() -> [RawWindow] {
        let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
            as? [[String: Any]] ?? []
        return list.compactMap { entry in
            guard let pid = entry[kCGWindowOwnerPID as String] as? Int32,
                  let dict = entry[kCGWindowBounds as String] as? NSDictionary,
                  let bounds = CGRect(dictionaryRepresentation: dict) else { return nil }
            return RawWindow(pid: pid, bounds: bounds, layer: entry[kCGWindowLayer as String] as? Int ?? 0)
        }
    }
}
