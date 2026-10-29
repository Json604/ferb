import CoreGraphics

enum IndicatorPlacement {
    static func safeTopX(proposedX: CGFloat, size: CGFloat, screen: CGRect, notch: CGRect) -> CGFloat {
        let x = min(screen.maxX - size, max(screen.minX, proposedX))
        guard x < notch.maxX && x + size > notch.minX else { return x }
        let left = max(screen.minX, notch.minX - size)
        let right = min(screen.maxX - size, notch.maxX)
        return abs(left - x) <= abs(right - x) ? left : right
    }
}
