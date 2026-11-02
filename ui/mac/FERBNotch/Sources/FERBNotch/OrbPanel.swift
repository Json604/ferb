import AppKit
import SwiftUI
import NotchCore

/// The orb's window: a transparent canvas centred on the orb, above the menu bar on every Space. It never takes a
/// click (the drag card is its own small window above it), never becomes key and never activates FERB, so the owner's
/// focus stays where it was (AGENTS.md rule 10).
final class OrbPanel: NSPanel, OrbSurface {
    static let canvas = CGSize(width: 600, height: 220)
    static let bubbleWidth: CGFloat = 240
    static let orbLevel = NSWindow.Level(rawValue: NSWindow.Level.mainMenu.rawValue + 2)
    /// As PanelCoordinator's overlay windows: every Space, beside full-screen apps, never in the window cycle.
    static let overlayBehavior: NSWindow.CollectionBehavior = [
        .canJoinAllSpaces, .canJoinAllApplications, .stationary,
        .ignoresCycle, .fullScreenAuxiliary, .fullScreenDisallowsTiling
    ]

    let state = OrbViewState()
    private let card = OrbCardPanel()
    private var center = CGPoint.zero
    var onDropAccepted: (() -> Void)? {
        get { card.cardView.onDropAccepted }
        set { card.cardView.onDropAccepted = newValue }
    }

    init() {
        super.init(contentRect: CGRect(origin: .zero, size: Self.canvas), styleMask: [.borderless, .nonactivatingPanel],
                   backing: .buffered, defer: true)
        title = "FERB guide"
        isOpaque = false
        backgroundColor = .clear
        hasShadow = false
        hidesOnDeactivate = false
        isReleasedWhenClosed = false
        canBecomeVisibleWithoutLogin = false
        level = Self.orbLevel
        collectionBehavior = Self.overlayBehavior
        animationBehavior = .none
        ignoresMouseEvents = true
        let hosting = NSHostingView(rootView: OrbCanvas(state: state))
        hosting.sizingOptions = []
        contentView = hosting
    }

    /// Always click-through: setting false still sets true, through AppKit's own setter.
    override var ignoresMouseEvents: Bool {
        get { super.ignoresMouseEvents }
        set { super.ignoresMouseEvents = true }
    }
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
    override func constrainFrameRect(_ frameRect: NSRect, to screen: NSScreen?) -> NSRect { frameRect }

    func place(center: CGPoint) {
        self.center = center
        setFrameOrigin(CGPoint(x: center.x - Self.canvas.width / 2, y: center.y - Self.canvas.height / 2))
        placeLogo()
        if state.instruction != nil { updateBubbleSide() }
    }

    func setAppearance(scale: CGFloat, opacity: CGFloat) {
        state.scale = scale
        state.opacity = opacity
        card.alphaValue = opacity
        placeLogo()
    }

    func setInstruction(_ line: IntroLine?) {
        state.instruction = line
        if line != nil { updateBubbleSide() }
    }

    func setDragItem(_ url: URL?) {
        card.cardView.url = url
        state.holding = url != nil
        card.ignoresMouseEvents = url == nil
    }

    func setWaiting(_ waiting: Bool) { state.waiting = waiting }

    func show() {
        orderFrontRegardless()
        showCard()
    }

    func hide() {
        hideCard()
        orderOut(nil)
    }

    private func placeLogo() {
        let width = OrbCardPanel.size.width * state.scale
        let height = OrbCardPanel.size.height * state.scale
        card.setFrame(CGRect(x: center.x - width / 2, y: center.y - height / 2,
                             width: width, height: height), display: true)
    }

    private func showCard() {
        if card.parent == nil { addChildWindow(card, ordered: .above) }
        card.orderFrontRegardless()
    }

    private func hideCard() {
        if card.parent != nil { removeChildWindow(card) }
        card.orderOut(nil)
    }

    /// The bubble sits toward the nearer edge of the orb's screen (away from the window the orb points at, which is
    /// usually nearer the middle), unless only the other side has room.
    private func updateBubbleSide() {
        let screens = NSScreen.screens.map(\.visibleFrame)
        guard let screen = screens.first(where: { $0.contains(center) }) ?? screens.first else { return }
        let reach = OrbController.size.width / 2 + 12 + Self.bubbleWidth
        let leftFits = center.x - reach >= screen.minX
        let rightFits = center.x + reach <= screen.maxX
        let left = center.x < screen.midX ? (leftFits || !rightFits) : (!rightFits && leftFits)
        if state.bubbleOnLeft != left { state.bubbleOnLeft = left }
    }
}

/// The small window holding the draggable FERB icon: the only part of the orb that takes the mouse. It never becomes
/// key and never activates FERB.
final class OrbCardPanel: NSPanel {
    static let size = OrbController.size
    let cardView = DragCardView(frame: CGRect(origin: .zero, size: OrbCardPanel.size))

    init() {
        super.init(contentRect: CGRect(origin: .zero, size: Self.size), styleMask: [.borderless, .nonactivatingPanel],
                   backing: .buffered, defer: true)
        title = "FERB logo"
        isOpaque = false
        backgroundColor = .clear
        hasShadow = false
        hidesOnDeactivate = false
        isReleasedWhenClosed = false
        canBecomeVisibleWithoutLogin = false
        level = OrbPanel.orbLevel
        collectionBehavior = OrbPanel.overlayBehavior
        animationBehavior = .none
        contentView = cardView
        ignoresMouseEvents = true
    }

    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
    override func constrainFrameRect(_ frameRect: NSRect, to screen: NSScreen?) -> NSRect { frameRect }
}
