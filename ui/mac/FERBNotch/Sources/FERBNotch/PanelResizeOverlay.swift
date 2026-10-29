import AppKit

/// A native, transparent border hit target. Only the perimeter returns itself from hit testing, so chat controls keep
/// receiving their normal events while every visible panel edge can start a resize with the matching cursor.
@MainActor
final class PanelResizeOverlay: NSView {
    override var isFlipped: Bool { true }
    private enum Direction {
        case left, right, top, bottom, topLeft, topRight, bottomLeft, bottomRight

        var horizontalSign: CGFloat {
            switch self {
            case .left, .topLeft, .bottomLeft: -1
            case .right, .topRight, .bottomRight: 1
            default: 0
            }
        }
        var verticalSign: CGFloat {
            switch self {
            case .top, .topLeft, .topRight: -1
            case .bottom, .bottomLeft, .bottomRight: 1
            default: 0
            }
        }
        var cursor: NSCursor {
            if #available(macOS 15.0, *) {
                let position: NSCursor.FrameResizePosition = switch self {
                case .left: .left
                case .right: .right
                case .top: .top
                case .bottom: .bottom
                case .topLeft: .topLeft
                case .topRight: .topRight
                case .bottomLeft: .bottomLeft
                case .bottomRight: .bottomRight
                }
                return .frameResize(position: position, directions: .all)
            }
            // macOS 14 exposes no public diagonal cursor. Keep Apple's native
            // horizontal cursor at corners; the gesture still resizes both axes.
            return horizontalSign == 0 ? .resizeUpDown : .resizeLeftRight
        }
    }

    /// In the backdrop's coordinates, so it remains correct as the surface morphs.
    var dragExclusionRect: CGRect? { didSet {
        if oldValue != dragExclusionRect { window?.invalidateCursorRects(for: self) }
    } }

    /// Injectable for offscreen cursor tests; production uses AppKit's native cursor.
    var applyCursor: (NSCursor) -> Void = { $0.set() }
    private var ownsCursor = false
    private var borderTracking: NSTrackingArea?
    private weak var model: NotchModel?
    private var activeDirection: Direction?
    private var dragOrigin = NSPoint.zero
    private var initialSize = CGSize.zero

    init(model: NotchModel) {
        self.model = model
        super.init(frame: .zero)
        autoresizingMask = []
        setAccessibilityElement(false)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let borderTracking { removeTrackingArea(borderTracking) }
        let area = NSTrackingArea(rect: .zero,
            options: [.activeAlways, .inVisibleRect, .mouseEnteredAndExited, .mouseMoved, .cursorUpdate],
            owner: self, userInfo: nil)
        addTrackingArea(area)
        borderTracking = area
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        window?.invalidateCursorRects(for: self)
    }

    override func setFrameOrigin(_ newOrigin: NSPoint) {
        super.setFrameOrigin(newOrigin)
        window?.invalidateCursorRects(for: self)
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        window?.invalidateCursorRects(for: self)
    }

    override func cursorUpdate(with event: NSEvent) { updateCursor(with: event) }
    override func mouseEntered(with event: NSEvent) { updateCursor(with: event) }
    override func mouseMoved(with event: NSEvent) { updateCursor(with: event) }
    override func mouseExited(with event: NSEvent) { releaseCursor() }

    private func updateCursor(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        guard model?.expanded == true, bounds.contains(point),
              dragExclusionRect?.contains(convert(point, to: superview)) != true,
              let direction = direction(at: point) else {
            releaseCursor()
            return
        }
        ownsCursor = true
        applyCursor(direction.cursor)
    }

    private func releaseCursor() {
        guard ownsCursor else { return }
        ownsCursor = false
        applyCursor(.arrow)
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        let local = convert(point, from: superview)
        guard dragExclusionRect?.contains(point) != true,
              model?.expanded == true, bounds.contains(local), direction(at: local) != nil else { return nil }
        return self
    }

    override func resetCursorRects() {
        super.resetCursorRects()
        guard model?.expanded == true else { return }
        let edge: CGFloat = 10
        let corner: CGFloat = 22
        addResizeCursorRect(CGRect(x: 0, y: corner, width: edge, height: max(0, bounds.height - 2 * corner)),
                      cursor: Direction.left.cursor)
        addResizeCursorRect(CGRect(x: bounds.maxX - edge, y: corner, width: edge,
                             height: max(0, bounds.height - 2 * corner)), cursor: Direction.right.cursor)
        addResizeCursorRect(CGRect(x: corner, y: 0, width: max(0, bounds.width - 2 * corner), height: edge),
                      cursor: Direction.top.cursor)
        addResizeCursorRect(CGRect(x: corner, y: bounds.maxY - edge, width: max(0, bounds.width - 2 * corner), height: edge),
                      cursor: Direction.bottom.cursor)
        addResizeCursorRect(CGRect(x: 0, y: 0, width: corner, height: corner), cursor: Direction.topLeft.cursor)
        addResizeCursorRect(CGRect(x: bounds.maxX - corner, y: 0, width: corner, height: corner),
                      cursor: Direction.topRight.cursor)
        addResizeCursorRect(CGRect(x: 0, y: bounds.maxY - corner, width: corner, height: corner),
                      cursor: Direction.bottomLeft.cursor)
        addResizeCursorRect(CGRect(x: bounds.maxX - corner, y: bounds.maxY - corner, width: corner, height: corner),
                      cursor: Direction.bottomRight.cursor)
    }

    private func addResizeCursorRect(_ rect: CGRect, cursor: NSCursor) {
        guard let exclusion = dragExclusionRect.map({ convert($0, from: superview) }),
              rect.intersects(exclusion) else {
            addCursorRect(rect, cursor: cursor)
            return
        }
        let overlap = rect.intersection(exclusion)
        for part in [CGRect(x: rect.minX, y: rect.minY, width: rect.width, height: overlap.minY - rect.minY),
                     CGRect(x: rect.minX, y: overlap.maxY, width: rect.width, height: rect.maxY - overlap.maxY),
                     CGRect(x: rect.minX, y: overlap.minY, width: overlap.minX - rect.minX, height: overlap.height),
                     CGRect(x: overlap.maxX, y: overlap.minY, width: rect.maxX - overlap.maxX, height: overlap.height)]
            where part.width > 0 && part.height > 0 {
            addCursorRect(part, cursor: cursor)
        }
    }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        guard let direction = direction(at: point), let model else { return }
        activeDirection = direction
        dragOrigin = screenPoint(event)
        initialSize = CGSize(width: model.bodyWidth, height: model.chatTranscriptHeight)
    }

    override func mouseDragged(with event: NSEvent) {
        guard let direction = activeDirection, let model else { return }
        let point = screenPoint(event)
        let dx = point.x - dragOrigin.x
        let dy = point.y - dragOrigin.y
        let horizontalFactor: CGFloat = ["top", "bottom"].contains(model.indicatorEdge) ? 2 : 1
        let verticalFactor: CGFloat = ["left", "right"].contains(model.indicatorEdge) ? 2 : 1
        model.resizePanel(width: initialSize.width + direction.horizontalSign * dx * horizontalFactor,
                          transcriptHeight: initialSize.height + direction.verticalSign * dy * verticalFactor)
    }

    override func mouseUp(with event: NSEvent) {
        guard activeDirection != nil, let model else { return }
        activeDirection = nil
        model.resizePanel(width: model.preferredPanelWidth,
                          transcriptHeight: model.preferredTranscriptHeight, persist: true)
        window?.resetCursorRects()
    }

    private func direction(at point: NSPoint) -> Direction? {
        let edge: CGFloat = 10
        let corner: CGFloat = 22
        let left = point.x <= edge
        let right = point.x >= bounds.maxX - edge
        let top = point.y <= edge
        let bottom = point.y >= bounds.maxY - edge
        if point.x <= corner && point.y <= corner { return .topLeft }
        if point.x >= bounds.maxX - corner && point.y <= corner { return .topRight }
        if point.x <= corner && point.y >= bounds.maxY - corner { return .bottomLeft }
        if point.x >= bounds.maxX - corner && point.y >= bounds.maxY - corner { return .bottomRight }
        if left { return .left }
        if right { return .right }
        if top { return .top }
        if bottom { return .bottom }
        return nil
    }

    private func screenPoint(_ event: NSEvent) -> NSPoint {
        let point = NSEvent.mouseLocation
        return NSPoint(x: point.x, y: -point.y)
    }
}
