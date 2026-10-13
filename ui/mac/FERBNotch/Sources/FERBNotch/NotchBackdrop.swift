import AppKit
import SwiftUI

private final class NotchTintView: NSView {
    var expansion: CGFloat = 0
    var cutoutHeight: CGFloat = 32
    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func draw(_ dirtyRect: NSRect) {
        guard let context = NSGraphicsContext.current?.cgContext else { return }
        context.saveGState()
        context.addPath(NotchSurfaceShape(expansion: expansion).path(in: bounds).cgPath)
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
    private var surfaceSize = CGSize.zero
    private var expansion: CGFloat = 0
    private var lastBounds = CGRect.zero
    override var isFlipped: Bool { true }

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        material = .popover
        blendingMode = .behindWindow
        state = .active
        appearance = NSAppearance(named: .darkAqua)
        maskImage = NSImage(size: NSSize(width: 1, height: 1), flipped: true) { _ in true }
        tint.wantsLayer = true
        addSubview(tint)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func updateSurface(size: CGSize, expansion: CGFloat, cutoutHeight: CGFloat) {
        surfaceSize = size
        self.expansion = expansion
        tint.expansion = expansion
        tint.cutoutHeight = cutoutHeight
        updateBackground()
    }

    override func layout() {
        super.layout()
        if lastBounds != bounds { updateBackground() }
    }

    private func updateBackground() {
        guard bounds.width > 0, bounds.height > 0 else { return }
        lastBounds = bounds
        let rect = CGRect(origin: .zero, size: surfaceSize)
        let x = (bounds.width - surfaceSize.width) / 2
        let path = NotchSurfaceShape(expansion: expansion).path(in: rect).offsetBy(dx: x, dy: 0).cgPath
        let visible = expansion > 0.001
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        tint.frame = CGRect(x: x, y: 0, width: surfaceSize.width, height: surfaceSize.height)
        tint.needsDisplay = true
        tint.displayIfNeeded()
        maskImage = NSImage(size: bounds.size, flipped: true) { _ in
            guard visible, let context = NSGraphicsContext.current?.cgContext else { return true }
            context.setFillColor(NSColor.white.cgColor)
            context.addPath(path)
            context.fillPath()
            return true
        }
        CATransaction.commit()
    }
}
