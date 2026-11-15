import CoreGraphics

enum IndicatorPlacement {
    /// Where on the top edge the handle rests: where the owner put it, kept on screen, and stepped aside only from a
    /// hardware notch's camera (`notch` nil on a notchless display: nothing to avoid, so it rests anywhere, the top
    /// centre included; live 2026-11-02 on an M1 it was pushed off centre by the virtual notch).
    static func safeTopX(proposedX: CGFloat, size: CGFloat, screen: CGRect, notch: CGRect?) -> CGFloat {
        let x = min(screen.maxX - size, max(screen.minX, proposedX))
        guard let notch, x < notch.maxX && x + size > notch.minX else { return x }
        let left = max(screen.minX, notch.minX - size)
        let right = min(screen.maxX - size, notch.maxX)
        return abs(left - x) <= abs(right - x) ? left : right
    }
}
