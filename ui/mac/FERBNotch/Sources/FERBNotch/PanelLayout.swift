import Foundation

/// Physical display bounds: the attached indicator remains on the screen edge,
/// including the menu-bar/camera strip. Layout budgets include transparent margins.
enum PanelLayout {
    static func surfaceBudget(screen: CGRect, edge: FERBScreenEdge, docked: Bool) -> CGSize {
        let side = !docked && (edge == .left || edge == .right)
        return CGSize(width: max(0, screen.width - 16 - (side ? 48 : 0)),
                      height: max(0, screen.height - 12 - (!docked && !side ? 28 : 0)))
    }

    static func frame(size: CGSize, screen: CGRect, indicator: CGRect, edge: FERBScreenEdge,
                      docked: Bool, cutout: CGRect) -> CGRect {
        let side = !docked && (edge == .left || edge == .right)
        let width = min(screen.width, size.width + (side ? 48 : 0))
        let height = min(screen.height, size.height + (!docked && !side ? 28 : 0))
        var origin: CGPoint
        if docked {
            origin = CGPoint(x: cutout.midX - width / 2, y: cutout.maxY - height)
        } else {
            origin = switch edge {
            case .top: CGPoint(x: indicator.midX - width / 2, y: screen.maxY - height)
            case .bottom: CGPoint(x: indicator.midX - width / 2, y: screen.minY)
            case .left: CGPoint(x: screen.minX, y: indicator.midY - height / 2)
            case .right: CGPoint(x: screen.maxX - width, y: indicator.midY - height / 2)
            }
        }
        origin.x = min(screen.maxX - width, max(screen.minX, origin.x))
        origin.y = min(screen.maxY - height, max(screen.minY, origin.y))
        return CGRect(origin: origin, size: CGSize(width: width, height: height))
    }

    static func draggedFrame(start: CGRect, grabbedAt: CGPoint, pointer: CGPoint) -> CGRect {
        start.offsetBy(dx: pointer.x - grabbedAt.x, dy: pointer.y - grabbedAt.y)
    }
}
