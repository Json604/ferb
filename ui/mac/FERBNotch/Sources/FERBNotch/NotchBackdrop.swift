import AppKit
import SwiftUI

private final class NotchTintView: NSView {
    var expansion: CGFloat = 0
    var docked = false
    var magneticJoin: CGFloat?
    var cutoutHeight: CGFloat = 32
    private let workspaceNotifications = NSWorkspace.shared.notificationCenter
    private var displayOptionsObserver: NSObjectProtocol?

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        displayOptionsObserver = workspaceNotifications.addObserver(
            forName: NSWorkspace.accessibilityDisplayOptionsDidChangeNotification,
            object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.needsDisplay = true }
        }
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    deinit {
        if let displayOptionsObserver { workspaceNotifications.removeObserver(displayOptionsObserver) }
    }

    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func draw(_ dirtyRect: NSRect) {
        guard let context = NSGraphicsContext.current?.cgContext else { return }
        context.saveGState()
        context.addPath(NotchSurfaceShape(expansion: expansion, docked: docked, joining: magneticJoin).path(in: bounds).cgPath)
        context.clip()
        if NSWorkspace.shared.accessibilityDisplayShouldReduceTransparency {
            NSColor.black.setFill()
            bounds.fill()
        } else {
            let neck = min(1, (cutoutHeight + 12) / max(bounds.height, 1))
            let gradient = NSGradient(colorsAndLocations:
                (NSColor.black, 0), (NSColor.black, neck),
                (NSColor.black.withAlphaComponent(0.64), neck + (1 - neck) * 0.32),
                (NSColor.black.withAlphaComponent(0.22), 1))!
            gradient.draw(from: .zero, to: CGPoint(x: 0, y: bounds.height), options: [])
            NSColor.black.withAlphaComponent(1 - expansion).setFill()
            bounds.fill()
        }
        context.restoreGState()
    }
}

/// Frost and its black tint are one native background. Both are updated together
/// from the presentation clock; SwiftUI only draws the controls above them.
final class NotchBackdropView: NSVisualEffectView {
    private let tint = NotchTintView()
    private let attachmentFill = CAShapeLayer()
    private let shoulderFill = CAShapeLayer()
    var attachmentRect: CGRect? { didSet { if oldValue != attachmentRect { updateBackground() } } }
    var attachmentEdge: FERBScreenEdge = .left { didSet { if oldValue != attachmentEdge { updateBackground() } } }
    var docked = false { didSet { if oldValue != docked { updateBackground() } } }
    var magneticJoin: CGFloat? { didSet { if oldValue != magneticJoin { updateBackground() } } }
    private var surfaceSize = CGSize.zero
    private var expansion: CGFloat = 0
    private var lastBounds = CGRect.zero
    private var updateDepth = 0
    private struct Background: Equatable {
        var bounds: CGRect
        var surface: CGRect
        var expansion: CGFloat
        var attachment: CGRect?
        var edge: FERBScreenEdge
        var docked: Bool
        var magneticJoin: CGFloat?
    }
    private var renderedBackground: Background?

    /// One drag frame changes the card and pill together. Build their final union
    /// once instead of rasterizing several intermediate attachment configurations.
    func performUpdates(_ updates: () -> Void) {
        updateDepth += 1
        defer {
            updateDepth -= 1
            if updateDepth == 0 { updateBackground() }
        }
        updates()
    }
    override var isFlipped: Bool { true }

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        material = .popover
        blendingMode = .behindWindow
        state = .active
        appearance = NSAppearance(named: .darkAqua)
        maskImage = NSImage(size: NSSize(width: 1, height: 1), flipped: true) { _ in true }
        wantsLayer = true
        attachmentFill.fillColor = NSColor.black.cgColor
        layer?.addSublayer(attachmentFill)
        shoulderFill.fillColor = NSColor.black.cgColor
        layer?.addSublayer(shoulderFill)
        tint.wantsLayer = true
        tint.layerContentsRedrawPolicy = .onSetNeedsDisplay
        addSubview(tint)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func updateSurface(size: CGSize, expansion: CGFloat, cutoutHeight: CGFloat, origin: CGPoint) {
        surfaceSize = size
        self.expansion = expansion
        if tint.expansion != expansion || tint.cutoutHeight != cutoutHeight {
            tint.expansion = expansion
            tint.cutoutHeight = cutoutHeight
            tint.needsDisplay = true
        }
        surfaceOrigin = origin
        updateBackground()
    }

    private var surfaceOrigin = CGPoint.zero

    override func layout() {
        super.layout()
        if lastBounds != bounds { updateBackground() }
    }

    private func updateBackground() {
        guard updateDepth == 0, bounds.width > 0, bounds.height > 0 else { return }
        let background = Background(bounds: bounds, surface: CGRect(origin: surfaceOrigin, size: surfaceSize),
                                    expansion: expansion, attachment: attachmentRect,
                                    edge: attachmentEdge, docked: docked, magneticJoin: magneticJoin)
        guard background != renderedBackground else { return }
        renderedBackground = background
        lastBounds = bounds
        let rect = CGRect(origin: .zero, size: surfaceSize)
        let cardPath = NotchSurfaceShape(expansion: expansion, docked: docked, joining: magneticJoin).path(in: rect)
            .offsetBy(dx: surfaceOrigin.x, dy: surfaceOrigin.y).cgPath
        let attachment = CGMutablePath()
        let shoulder = CGMutablePath()
        if let pill = attachmentRect {
            attachment.addRoundedRect(in: pill, cornerWidth: 15, cornerHeight: 15)
            let card = CGRect(origin: surfaceOrigin, size: surfaceSize)
            shoulder.addPath(PillShoulder.path(pill: pill, card: card, edge: attachmentEdge, expansion: expansion))
        }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        attachmentFill.path = attachment
        shoulderFill.path = shoulder
        let tintNeedsPaint = tint.frame.size != surfaceSize || tint.docked != docked || tint.magneticJoin != magneticJoin
        tint.magneticJoin = magneticJoin
        tint.docked = docked
        let pendingTintPaint = tint.needsDisplay
        if tint.frame != background.surface { tint.frame = background.surface }
        // AppKit dirties a moved view, although this gradient is entirely local
        // to its bounds. Keep its retained pixels and any genuinely pending paint.
        if let resize = subviews.first(where: { $0 is PanelResizeOverlay }) {
            if resize.frame != tint.frame { resize.frame = tint.frame }
        }
        maskImage = NSImage(size: bounds.size, flipped: true) { _ in
            guard let context = NSGraphicsContext.current?.cgContext else { return true }
            context.setFillColor(NSColor.white.cgColor)
            // Paint the union independently: mirrored shoulders have opposite winding.
            for component in [cardPath, attachment, shoulder] {
                context.addPath(component)
                context.fillPath()
            }
            return true
        }
        tint.needsDisplay = pendingTintPaint || tintNeedsPaint
        CATransaction.commit()
    }
}

/// Shared-surface coordinates, with inward pointing toward the conversation.
enum PillShoulder {
    static func path(pill: CGRect, card: CGRect, edge: FERBScreenEdge, expansion: CGFloat) -> CGPath {
        let shoulder = CGMutablePath()
        // Work in coordinates pointing inward from the chosen screen edge.
        let horizontal = edge == .left || edge == .right
        let positive = edge == .left || edge == .top
        func point(_ inward: CGFloat, _ along: CGFloat) -> CGPoint {
            horizontal ? CGPoint(x: positive ? inward : -inward, y: along)
                   : CGPoint(x: along, y: positive ? inward : -inward)
        }
        let boundary: CGFloat = horizontal
            ? (positive ? card.minX : -card.maxX)
            : (positive ? card.minY : -card.maxY)
        let start: CGFloat = horizontal
            ? (positive ? pill.midX : -pill.midX)
            : (positive ? pill.midY : -pill.midY)
        let low = horizontal ? pill.minY : pill.minX
        let high = horizontal ? pill.maxY : pill.maxX
        let tip = horizontal ? (positive ? pill.maxX : -pill.minX)
                         : (positive ? pill.maxY : -pill.minY)
        // The spring moves the card boundary through the stationary pill. Its
        // exposed shoulder grows from zero as it passes the pill's center,
        // rather than appearing at full size when the boundary nears the tip.
        // Constrain the curve to current overlap: it must never create a bridge.
        let cardLow = horizontal ? card.minY : card.minX
        let cardHigh = horizontal ? card.maxY : card.maxX
        let radius = max(0, min(12 * min(1, max(0, expansion)),
                                boundary - start, low - cardLow, cardHigh - high))
        if radius > 0, boundary <= tip {
            shoulder.move(to: point(start, low))
            shoulder.addLine(to: point(boundary - radius, low))
            shoulder.addQuadCurve(to: point(boundary, low - radius),
                            control: point(boundary, low))
            shoulder.addLine(to: point(boundary + 1, low - radius))
            shoulder.addLine(to: point(boundary + 1, high + radius))
            shoulder.addLine(to: point(boundary, high + radius))
            shoulder.addQuadCurve(to: point(boundary - radius, high),
                            control: point(boundary, high))
            shoulder.addLine(to: point(start, high))
            shoulder.closeSubpath()
        }
        return shoulder
    }
}
