import CoreGraphics

/// What FERB's layout depends on, per display: its identity, frame, camera housing (the top safe-area inset and the
/// areas beside the housing) and backing scale. A Space change, a full-screen app or a menu bar that hides itself
/// changes none of these, so a screen-parameters notice with the same layout leaves FERB exactly as it is (APP-729).
/// The visible frame (menu bar and Dock) is deliberately not part of it.
public struct ScreenLayout: Equatable, Sendable {
    public struct Display: Equatable, Sendable {
        public var id: UInt32?
        public var frame: CGRect
        public var topInset: CGFloat
        public var leftArea: CGRect?
        public var rightArea: CGRect?
        public var scale: CGFloat

        public init(id: UInt32?, frame: CGRect, topInset: CGFloat, leftArea: CGRect?, rightArea: CGRect?,
                    scale: CGFloat) {
            self.id = id
            self.frame = frame
            self.topInset = topInset
            self.leftArea = leftArea
            self.rightArea = rightArea
            self.scale = scale
        }
    }

    public var displays: [Display]

    public init(displays: [Display]) { self.displays = displays }
}

/// The menu bar's height on a display, steady through Spaces: the gap above the visible frame while the menu bar
/// shows, else the height last seen on that display (a full-screen Space or an auto-hiding menu bar reports 0), else
/// nothing known (NotchGeometry then uses its floor).
public struct MenuBarHeights: Sendable {
    private var seen: [UInt32: CGFloat] = [:]

    public init() {}

    public mutating func height(display: UInt32?, visibleGap: CGFloat) -> CGFloat {
        if visibleGap > 0 {
            if let display { seen[display] = visibleGap }
            return visibleGap
        }
        return display.flatMap { seen[$0] } ?? 0
    }
}

public enum SpaceFacts {
    /// SkyLight's type for a full-screen app's Space (a desktop is 0), as SLSSpaceGetType reports it.
    public static let fullScreenType: Int32 = 4

    /// Mission Control (or App Exposé) is showing: the window manager process draws a window above the normal layer
    /// that covers the whole display (window-server coordinates). Stage Manager's strip and the Dock (its own
    /// full-display window sits on the Dock layer, always) don't.
    public static func missionControlShowing(in rows: [ScreenWindow], display: CGRect,
                                             isWindowManager: (Int32) -> Bool) -> Bool {
        rows.contains { row in
            row.layer > 0 && row.layer != NotchOverlap.dockLayer && row.alpha >= NotchOverlap.minimumVisibleAlpha
                && row.bounds.insetBy(dx: -1, dy: -1).contains(display) && isWindowManager(row.ownerPID)
        }
    }
}
