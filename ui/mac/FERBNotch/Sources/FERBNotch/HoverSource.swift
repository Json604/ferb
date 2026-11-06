import AppKit
import OSLog
import SwiftUI
import NotchCore

// APP-753, APP-756: the one hover source for everything in the chat a click opens or reveals. FERB's notch and sticky
// window are non-activating panels and FERB is almost never the active app, so SwiftUI's `.onHover` (whose tracking
// follows the active app) never fires there. This is AppKit tracking that is always active, the way FERB's handle
// and resize edges track the pointer: entering and leaving drive the hover colour (`HoverLook`), and an openable
// thing shows the pointing hand, set again on every move and put back to the arrow when the pointer leaves, the view
// goes away or its window stops being visible, so the hand is never left behind.

/// The tracking view: invisible, never takes a click (hit-testing passes through to the card below it).
final class HoverSourceView: NSView {
    static let log = Logger(subsystem: "com.ferb.FERB", category: "hover")
    var onChange: ((Bool) -> Void)?
    /// Shows the pointing hand while the pointer is over it (anything a click opens or reveals).
    var pointing = false
    /// A short name for the log line (never content): "file", "site", "more", "link", "reminder" …
    var label = ""
    private(set) var hovered = false
    /// The cursor this view last asked AppKit for (tests read it; AppKit owns the real one).
    private(set) var cursorAsked: NSCursor?
    private var area: NSTrackingArea?
    private var watched: NSObjectProtocol?

    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    override var acceptsFirstResponder: Bool { false }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let area { removeTrackingArea(area) }
        let made = NSTrackingArea(rect: .zero, options: [.activeAlways, .mouseEnteredAndExited, .mouseMoved,
                                                         .cursorUpdate, .inVisibleRect], owner: self, userInfo: nil)
        addTrackingArea(made)
        area = made
    }

    override func mouseEntered(with event: NSEvent) { set(true) }
    override func mouseMoved(with event: NSEvent) {
        if !hovered { set(true) }
        if pointing { show(.pointingHand) }               // the window server may have put the arrow back
    }
    override func mouseExited(with event: NSEvent) { set(false) }
    override func cursorUpdate(with event: NSEvent) {
        if pointing && hovered { show(.pointingHand) } else { super.cursorUpdate(with: event) }
    }

    override func resetCursorRects() {
        super.resetCursorRects()
        if pointing { addCursorRect(bounds, cursor: .pointingHand) }   // AppKit's own, while the window is key
    }

    override func viewWillMove(toWindow newWindow: NSWindow?) {
        super.viewWillMove(toWindow: newWindow)
        if newWindow !== window { set(false) }
        if let watched { NotificationCenter.default.removeObserver(watched) }
        watched = nil
        guard let newWindow else { return }
        watched = NotificationCenter.default.addObserver(forName: NSWindow.didChangeOcclusionStateNotification,
                                                         object: newWindow, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated {
                if newWindow.occlusionState.contains(.visible) == false || !newWindow.isVisible { self?.set(false) }
            }
        }
    }

    override func viewDidHide() {
        super.viewDidHide()
        set(false)
    }

    private func show(_ cursor: NSCursor) {
        cursor.set()
        cursorAsked = cursor
    }

    deinit {
        if let watched { NotificationCenter.default.removeObserver(watched) }
    }

    /// The hover changed: tell the card, set or put back the cursor, and say so in the log (state only).
    func set(_ inside: Bool) {
        guard inside != hovered else { return }
        hovered = inside
        if pointing { show(inside ? .pointingHand : .arrow) }
        Self.log.info("hover \(inside ? "on" : "off", privacy: .public) \(self.label, privacy: .public) pointing=\(self.pointing, privacy: .public)")
        onChange?(inside)
    }
}

struct HoverArea: NSViewRepresentable {
    let pointing: Bool
    let label: String
    let onChange: (Bool) -> Void

    func makeNSView(context: Context) -> HoverSourceView { HoverSourceView() }
    func updateNSView(_ view: HoverSourceView, context: Context) {
        view.onChange = onChange
        view.label = label
        if view.pointing != pointing {
            view.pointing = pointing
            view.window?.invalidateCursorRects(for: view)
        }
    }
}

/// The shared hover look (APP-753, `HoverLook`) on the AppKit hover source: the card's fill brightens under the
/// pointer, a link's text lifts towards white (the caller reads `hovered`), and an openable thing shows the pointing
/// hand (APP-756); only colour and cursor change, nothing opens (APP-715). Reduce Motion changes it at once.
struct CardHover: ViewModifier {
    var link = false
    var cornerRadius: CGFloat = 9
    var capsule = false
    /// Draws the fill; off when the content draws its own from `hovered`.
    var drawsFill = true
    var label = "card"
    let hovered: Binding<Bool>
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        content
            .background {
                if drawsFill {
                    if capsule { Capsule().fill(.white.opacity(HoverLook.fill(hovered: hovered.wrappedValue))) }
                    else {
                        RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                            .fill(.white.opacity(HoverLook.fill(hovered: hovered.wrappedValue)))
                    }
                }
            }
            .overlay {
                HoverArea(pointing: link, label: label) { inside in
                    withAnimation(HoverLook.duration(reduceMotion: reduceMotion).map { .easeOut(duration: $0) }) {
                        hovered.wrappedValue = inside
                    }
                }
            }
    }
}

extension View {
    /// The hover colour and, for something a click opens or reveals (`link`), the pointing hand.
    func cardHover(_ hovered: Binding<Bool>, link: Bool = false, cornerRadius: CGFloat = 9, capsule: Bool = false,
                   drawsFill: Bool = true, label: String = "card") -> some View {
        modifier(CardHover(link: link, cornerRadius: cornerRadius, capsule: capsule, drawsFill: drawsFill, label: label,
                           hovered: hovered))
    }

    /// Only the pointing hand (APP-756), for something a click opens that has no hover colour of its own.
    func pointingHand(_ label: String) -> some View {
        overlay { HoverArea(pointing: true, label: label) { _ in } }
    }
}

extension NSColor {
    /// A link's colour under the pointer (`HoverLook.linkLift`).
    func hoverLifted(_ hovered: Bool) -> NSColor {
        let lift = HoverLook.linkLift(hovered: hovered)
        return lift > 0 ? (blended(withFraction: lift, of: .white) ?? self) : self
    }
}
