import Foundation
import CoreGraphics

/// AppKit screen coordinates are points with their origin at the bottom left.
public struct NotchGeometry {
    public let centerX: CGFloat
    public let topEdge: CGFloat
    public let handleWidth: CGFloat
    public let hasNotch: Bool

    public init(screen: CGRect, topInset: CGFloat, leftAreaMaxX: CGFloat?,
                rightAreaMinX: CGFloat?, menuBarHeight: CGFloat) {
        hasNotch = topInset > 0 && leftAreaMaxX != nil && rightAreaMinX != nil
        if hasNotch, let left = leftAreaMaxX, let right = rightAreaMinX, right > left {
            centerX = (left + right) / 2
            handleWidth = right - left
        } else {
            centerX = screen.midX
            handleWidth = 164
        }
        topEdge = screen.maxY - (hasNotch ? topInset : max(menuBarHeight, 0))
    }

    public func panelFrame(size: CGSize) -> CGRect {
        CGRect(x: centerX - size.width / 2, y: topEdge - size.height,
               width: size.width, height: size.height)
    }
}
