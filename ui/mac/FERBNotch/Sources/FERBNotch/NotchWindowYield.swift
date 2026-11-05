import AppKit

/// What yielding may do to a notch window: order it out, in and relative to another window, set its level, and
/// give it keyboard focus. Deliberately nothing about mouse events. Once `ignoresMouseEvents` has been set on a
/// window, even back to false, AppKit stops letting clicks through its transparent areas, so the panel's
/// transparent margins (the strip under the folded pinned header, the side and bottom margins) would start
/// swallowing clicks meant for apps underneath.
@MainActor
protocol NotchYieldWindow: AnyObject {
    var isVisible: Bool { get }
    var isKeyWindow: Bool { get }
    var level: NSWindow.Level { get set }
    func orderOut(_ sender: Any?)
    func orderFrontRegardless()
    func order(_ place: NSWindow.OrderingMode, relativeTo otherWindowNumber: Int)
    func makeKey()
}

extension NSWindow: NotchYieldWindow {}

/// Applies a yield to FERB's two windows. While FERB yields, the panel steps one layer back rather than leaving
/// the screen (owner, 2026-10-24: "take it one layer behind rather than closing it back"). It leaves the fixed
/// overlay space and becomes an ordinary window on the normal level, just behind the app window it yields to, or,
/// yielding to a menu, popover or floating window (all above the normal level), in front of ordinary windows. So it
/// stays on screen for screenshots and while a clipboard app pastes into it, and a click on it brings it back.
/// Coming back puts it at its own level in the overlay space again. The light is ordered out while FERB yields,
/// except while the amber attention light is on: that window only ever draws the glow and has never taken a click,
/// so a question that arrives while FERB yields is still seen.
@MainActor
final class NotchWindowYield {
    private let panel: NotchYieldWindow
    private let tracer: NotchYieldWindow
    private let rejoinOverlay: (NotchYieldWindow) -> Void
    private let leaveOverlay: (NotchYieldWindow) -> Void
    private var keyBeforeYield = false
    /// The panel's own level, kept while it sits on the normal level.
    private var ownLevel: NSWindow.Level?
    /// The window the panel sits just behind while it yields; nil: in front of ordinary windows.
    private(set) var behind: Int?
    private(set) var isYielded = false

    init(panel: NotchYieldWindow, tracer: NotchYieldWindow, rejoinOverlay: @escaping (NotchYieldWindow) -> Void,
         leaveOverlay: @escaping (NotchYieldWindow) -> Void = { _ in }) {
        self.panel = panel
        self.tracer = tracer
        self.rejoinOverlay = rejoinOverlay
        self.leaveOverlay = leaveOverlay
    }

    /// Steps the panel one layer back, just behind window `behind` (another app's window under the pinned chat), or
    /// in front of ordinary windows when nil (a menu, popover or floating window over FERB, or FERB's file picker).
    /// A panel that is out (FERB closed, nothing of its surface showing) stays out: it is only marked as yielding,
    /// so it can't appear as an empty black window (APP-729).
    func yield(behind window: Int?) {
        guard !isYielded else { return }
        isYielded = true
        keyBeforeYield = panel.isKeyWindow
        if ownLevel == nil { ownLevel = panel.level }
        leaveOverlay(panel)
        panel.level = .normal
        behind = window
        if panel.isVisible { place(behind: window) }
    }

    /// While FERB yields, what it yields to changed (another app came forward, the app's front window changed, a
    /// menu took over from the app): sit behind that instead. Nothing moves when it's the same.
    func restack(behind window: Int?) {
        guard isYielded, window != behind else { return }
        behind = window
        if panel.isVisible { place(behind: window) }
    }

    /// The owner clicked or switched into another app while FERB yielded: coming back must not take the focus
    /// they moved there.
    func ownerMovedElsewhere() {
        if isYielded { keyBeforeYield = false }
    }

    /// Puts the panel back at its own level in the overlay space. It takes keyboard focus again only if it had it
    /// when the yield began, the owner stayed (`ownerMovedElsewhere` wasn't called) and `refocus` allows it (body
    /// shown, no held voice chord).
    func comeBack(refocus: Bool) {
        guard isYielded else { return }
        isYielded = false
        behind = nil
        if let ownLevel { panel.level = ownLevel }
        // Back at its own level in the overlay space; a panel that is out (FERB closed meanwhile) stays out.
        if panel.isVisible { panel.orderFrontRegardless() }
        rejoinOverlay(panel)
        if keyBeforeYield && refocus { panel.makeKey() }
        keyBeforeYield = false
    }

    /// The owner's hover, click on the notch or shortcut brought FERB back: focus the visible text input, as hover
    /// and double-tap always do. Never while FERB yields: a panel behind another app must not take keyboard focus.
    /// Returns whether the panel took focus.
    func focusInput(bodyVisible: Bool) -> Bool {
        guard !isYielded, bodyVisible else { return false }
        panel.makeKey()
        return true
    }

    /// Keeps the panel on screen after a presentation, Space change or wake: callers call it only while FERB's
    /// surface shows (APP-729). While FERB yields it stays where it stepped back to, and is placed there again only if
    /// something took it off the screen.
    func showPanel() {
        if !isYielded { orderIn(panel) }
        else if !panel.isVisible { place(behind: behind) }
    }

    /// `lightShows` is false while FERB yields, unless the attention light is on.
    func updateTracer(glowing: Bool, lightShows: Bool) {
        if glowing && lightShows { orderIn(tracer) } else { tracer.orderOut(nil) }
    }

    private func place(behind window: Int?) {
        behind = window
        if let window { panel.order(.below, relativeTo: window) } else { panel.orderFrontRegardless() }
    }

    /// Ordered in, then kept in the overlay space and on no ordinary Space (`rejoinOverlay` moves it only if needed).
    private func orderIn(_ window: NotchYieldWindow) {
        window.orderFrontRegardless()
        rejoinOverlay(window)
    }
}
