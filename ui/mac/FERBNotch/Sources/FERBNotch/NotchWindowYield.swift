import AppKit

/// What yielding may do to a notch window: order it out and back in, and give it keyboard focus. Deliberately
/// nothing about mouse events. Once `ignoresMouseEvents` has been set on a window, even back to false, AppKit stops
/// letting clicks through its transparent areas, so the panel's transparent margins (the strip under the folded
/// pinned header, the side and bottom margins) would start swallowing clicks meant for apps underneath.
@MainActor
protocol NotchYieldWindow: AnyObject {
    var isVisible: Bool { get }
    var isKeyWindow: Bool { get }
    func orderOut(_ sender: Any?)
    func orderFrontRegardless()
    func makeKey()
}

extension NSWindow: NotchYieldWindow {}

/// Applies a yield to FERB's two windows. While FERB yields the panel is ordered out (invisible, takes no clicks,
/// holds no keyboard focus) and comes back in the fixed overlay space. The light is ordered out too, except while
/// the amber attention light is on: that window only ever draws the glow and has never taken a click, so a
/// question that arrives while FERB yields is still seen.
@MainActor
final class NotchWindowYield {
    private let panel: NotchYieldWindow
    private let tracer: NotchYieldWindow
    private let rejoinOverlay: (NotchYieldWindow) -> Void
    private var keyBeforeYield = false
    private(set) var isYielded = false

    init(panel: NotchYieldWindow, tracer: NotchYieldWindow, rejoinOverlay: @escaping (NotchYieldWindow) -> Void) {
        self.panel = panel
        self.tracer = tracer
        self.rejoinOverlay = rejoinOverlay
    }

    func yield() {
        guard !isYielded else { return }
        isYielded = true
        keyBeforeYield = panel.isKeyWindow
        panel.orderOut(nil)
    }

    /// The owner clicked or switched into another app while FERB yielded: coming back must not take the focus
    /// they moved there.
    func ownerMovedElsewhere() {
        if isYielded { keyBeforeYield = false }
    }

    /// Orders the panel back in. It takes keyboard focus again only if it had it when the yield began, the owner
    /// stayed (`ownerMovedElsewhere` wasn't called) and `refocus` allows it (body shown, no held voice chord).
    func comeBack(refocus: Bool) {
        guard isYielded else { return }
        isYielded = false
        orderIn(panel)
        if keyBeforeYield && refocus { panel.makeKey() }
        keyBeforeYield = false
    }

    /// The owner's hover, click on the notch or shortcut brought FERB back: focus the visible text input, as hover
    /// and double-tap always do. Never while FERB yields: an ordered-out panel must not take keyboard focus.
    /// Returns whether the panel took focus.
    func focusInput(bodyVisible: Bool) -> Bool {
        guard !isYielded, bodyVisible else { return false }
        panel.makeKey()
        return true
    }

    /// Keeps the panel on screen after a presentation, Space change or wake, except while FERB yields.
    func showPanel() {
        if !isYielded { orderIn(panel) }
    }

    /// `lightShows` is false while FERB yields, unless the attention light is on.
    func updateTracer(glowing: Bool, lightShows: Bool) {
        if glowing && lightShows { orderIn(tracer) } else { tracer.orderOut(nil) }
    }

    private func orderIn(_ window: NotchYieldWindow) {
        let wasOut = !window.isVisible
        window.orderFrontRegardless()
        if wasOut { rejoinOverlay(window) }
    }
}
