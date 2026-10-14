import CoreGraphics
import NotchCore

/// Window-server coordinates modelled on a 14-inch notched MacBook: 1470 × 956 points, a 33-point menu bar and a
/// 179-point camera cutout at the top centre.
enum YieldFixture {
    static let display = CGRect(x: 0, y: 0, width: 1470, height: 956)
    static let menuBar = CGRect(x: 0, y: 0, width: 1470, height: 33)
    static let notch = CGRect(x: 646, y: -2, width: 179, height: 36)
    /// The panel window (surface plus its transparent margin) for a folded pinned header and for the open chat.
    static let headerPanel = CGRect(x: 547, y: 0, width: 376, height: 44)
    static let openPanel = CGRect(x: 547, y: 0, width: 376, height: 432)
    /// The drawn surfaces: the folded header stays inside the menu bar; the open chat reaches down over windows.
    static let headerSurface = CGRect(x: 555, y: 0, width: 360, height: 32)
    static let openSurface = CGRect(x: 555, y: 0, width: 360, height: 420)
    static let ferb: Int32 = 76216
    static let windowServer: Int32 = 170
    static let clipboardApp: Int32 = 900
    static let browser: Int32 = 600
    static let finder: Int32 = 700

    /// Without an explicit `id`, the same owner, layer and bounds give the same window number (the same window).
    static func window(_ pid: Int32, layer: Int, _ x: CGFloat, _ y: CGFloat, _ width: CGFloat, _ height: CGFloat,
                       alpha: Double = 1, id: UInt32? = nil) -> ScreenWindow {
        var hasher = Hasher()
        for value in [CGFloat(pid), CGFloat(layer), x, y, width, height] { hasher.combine(value) }
        return ScreenWindow(windowID: id ?? UInt32(truncatingIfNeeded: hasher.finalize()), ownerPID: pid, layer: layer,
                            bounds: CGRect(x: x, y: y, width: width, height: height), alpha: alpha)
    }

    /// A status-item dropdown just right of the notch, at the pop-up menu layer.
    static let clipboardMenu = window(clipboardApp, layer: 101, 850, 33, 260, 400)
    static let maximizedBrowser = window(browser, layer: 0, 0, 33, 1470, 923)
    /// A floating window that never closes, just below the menu bar at the top centre (a screen-share control bar).
    static let shareBar = window(800, layer: 3, 600, 36, 280, 40)
}
