import AppKit
import Combine

enum FERBScreenEdge: String { case top, right, bottom, left }

/// The movable, compact FERB handle lives on the chosen display edge while the conversation panel is closed.
final class FerbIndicatorPanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
    override func constrainFrameRect(_ frameRect: NSRect, to screen: NSScreen?) -> NSRect { frameRect }
}

final class FerbIndicatorControl: NSView {
    var onDragBegan: ((CGPoint) -> Void)?
    var onDragged: ((CGPoint) -> Void)?
    var onDragEnded: ((CGPoint) -> Void)?
    var onClick: (() -> Void)?
    var onPointerChanged: (() -> Void)?
    private var pointerTracking: NSTrackingArea?
    private var dragBeganAt: CGPoint?
    private var isDragging = false
    private let model: NotchModel
    private var observation: AnyCancellable?
    private var isResting = false
    var isDocked = false { didSet { if oldValue != isDocked { refreshAppearance() } } }
    var isAttached = false { didSet { if oldValue != isAttached { refreshAppearance() } } }
    private let logo = NSImageView(frame: CGRect(x: 5, y: 4, width: 22, height: 22))
    private let statusDot = NSView(frame: CGRect(x: 45, y: 12, width: 6, height: 6))
    private let homeLine = NSView()

    init(model: NotchModel) {
        self.model = model
        super.init(frame: CGRect(x: 0, y: 0, width: 58, height: 30))
        wantsLayer = true
        layer?.cornerRadius = 15
        layer?.borderWidth = 0
        if let url = Bundle.module.url(forResource: "ferb-mark", withExtension: "svg"),
           let image = NSImage(contentsOf: url) {
            logo.image = image
        }
        logo.imageScaling = .scaleProportionallyUpOrDown
        logo.wantsLayer = true
        logo.layer?.cornerRadius = 5
        logo.layer?.masksToBounds = true
        addSubview(logo)
        homeLine.wantsLayer = true
        homeLine.layer?.cornerRadius = 2
        homeLine.layer?.backgroundColor = NSColor.white.withAlphaComponent(0.72).cgColor
        homeLine.alphaValue = 0
        addSubview(homeLine)
        statusDot.wantsLayer = true
        statusDot.layer?.cornerRadius = 3
        addSubview(statusDot)
        observation = model.objectWillChange.sink { [weak self] _ in
            DispatchQueue.main.async { self?.refreshAppearance() }
        }
        refreshAppearance()
        setAccessibilityElement(true)
        setAccessibilityRole(.button)
        setAccessibilityLabel("Open FERB")
        setAccessibilityHelp("Drag FERB to another edge of the screen")
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func setMagneticFusion(_ progress: CGFloat) {
        // Content sinks into the same join as the actual black capsule.
        logo.alphaValue = 1 - progress
        statusDot.alphaValue = 1 - progress
    }

    func setResting(_ resting: Bool, animated: Bool) {
        let animated = animated && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        guard isResting != resting else { refreshAppearance(); return }
        isResting = resting
        refreshAppearance()
        let changes = {
            (animated ? self.logo.animator() : self.logo).alphaValue = resting ? 0 : 1
            (animated ? self.statusDot.animator() : self.statusDot).alphaValue = resting ? 0 : 1
            (animated ? self.homeLine.animator() : self.homeLine).alphaValue = resting ? 1 : 0
        }
        if animated {
            NSAnimationContext.runAnimationGroup { context in
                context.duration = 0.34
                context.timingFunction = CAMediaTimingFunction(controlPoints: 0.2, 0.8, 0.2, 1)
                changes()
            }
        } else {
            changes()
        }
        setAccessibilityLabel("Open FERB")
    }

    override func updateTrackingAreas() {
        if let pointerTracking { removeTrackingArea(pointerTracking) }
        let area = NSTrackingArea(rect: .zero,
                                  options: [.activeAlways, .mouseEnteredAndExited, .mouseMoved, .inVisibleRect],
                                  owner: self, userInfo: nil)
        addTrackingArea(area)
        pointerTracking = area
        super.updateTrackingAreas()
    }

    // The camera control is a non-key window, then gets reparented into the
    // open chat. Native tracking follows both without relying solely on global
    // mouseMoved delivery (which excludes events delivered to FERB itself).
    override func mouseEntered(with event: NSEvent) { onPointerChanged?() }
    override func mouseExited(with event: NSEvent) { onPointerChanged?() }
    override func mouseMoved(with event: NSEvent) { onPointerChanged?() }

    override func layout() {
        super.layout()
        layoutIndicatorContent()
    }

    private func layoutIndicatorContent() {
        logo.frame = CGRect(x: isResting ? (bounds.width - 22) / 2 : 5, y: 4, width: 22, height: 22)
        statusDot.frame = CGRect(x: bounds.width - 13, y: 12, width: 6, height: 6)
        // Fill to the physical screen edge; only the inward corners are rounded.
        homeLine.frame = bounds
        homeLine.layer?.cornerRadius = 2.5
        switch model.indicatorEdge {
        case "left": homeLine.layer?.maskedCorners = [.layerMaxXMinYCorner, .layerMaxXMaxYCorner]
        case "right": homeLine.layer?.maskedCorners = [.layerMinXMinYCorner, .layerMinXMaxYCorner]
        case "top": homeLine.layer?.maskedCorners = [.layerMinXMinYCorner, .layerMaxXMinYCorner]
        default: homeLine.layer?.maskedCorners = [.layerMinXMaxYCorner, .layerMaxXMaxYCorner]
        }
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        refreshAppearance()
    }

    override func accessibilityPerformPress() -> Bool {
        onClick?()
        return true
    }

    private func refreshAppearance() {
        let glow = model.showsTraceGlow
        logo.isHidden = isDocked
        statusDot.isHidden = isDocked
        homeLine.isHidden = isDocked
        setAccessibilityHelp(isDocked ? "Drag below the notch to detach FERB" : "Drag FERB to another edge or onto the Mac notch")
        effectiveAppearance.performAsCurrentDrawingAppearance {
            homeLine.layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
        }
        layoutIndicatorContent()
        // The idle bar has no material capsule, outline, shadow, or status tint.
        layer?.cornerRadius = isResting ? 0 : 15
        layer?.borderWidth = 0
        layer?.backgroundColor = (isResting || isAttached || isDocked) ? nil : NSColor.black.cgColor
        layer?.borderColor = (glow ? NSColor.systemYellow : NSColor.white.withAlphaComponent(0.18)).cgColor
        switch model.indicatorPhase {
        case .working: statusDot.layer?.backgroundColor = NSColor.systemYellow.cgColor
        case .attention: statusDot.layer?.backgroundColor = NSColor.systemRed.cgColor
        case .result: statusDot.layer?.backgroundColor = NSColor.systemGreen.cgColor
        case .ready, .draft, .listening, .stopped: statusDot.layer?.backgroundColor = NSColor.white.cgColor
        }
    }

    // Window-local event positions may have been queued before this moving window's
    // latest frame. Sample the global pointer instead of adding a newer window origin.
    var pointerLocation: () -> CGPoint = { NSEvent.mouseLocation }
    private func screenPoint(_ event: NSEvent) -> CGPoint { pointerLocation() }

    override func mouseDown(with event: NSEvent) {
        let point = screenPoint(event)
        dragBeganAt = point
        isDragging = false
        onDragBegan?(point)
    }

    override func mouseDragged(with event: NSEvent) {
        let point = screenPoint(event)
        if let start = dragBeganAt, hypot(point.x - start.x, point.y - start.y) >= 3 { isDragging = true }
        if isDragging { onDragged?(point) }
    }

    override func mouseUp(with event: NSEvent) {
        let point = screenPoint(event)
        if isDragging { onDragEnded?(point) } else { onClick?() }
        dragBeganAt = nil
        isDragging = false
    }
}
