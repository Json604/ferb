import Foundation
import CoreGraphics

/// Screen coordinates: the camera housing and the reachable strip immediately below it.
enum NotchDocking {
    static func pullStrip(cutout: CGRect) -> CGRect {
        CGRect(x: cutout.minX, y: cutout.minY - 12, width: cutout.width, height: 12)
    }

    static func dragRegion(cutout: CGRect) -> CGRect {
        cutout.union(pullStrip(cutout: cutout))
    }

    static func accepts(_ point: CGPoint, cutout: CGRect, hasHardwareNotch: Bool) -> Bool {
        guard hasHardwareNotch else { return false }
        let region = dragRegion(cutout: cutout)
        // CGRect.contains excludes maxY, but the physical screen-top pixel is
        // reachable and belongs to the camera's hover target too.
        return point.x >= region.minX && point.x <= region.maxX
            && point.y >= region.minY && point.y <= region.maxY
    }

    /// Applied around the actual AppKit layer anchor; top-left remains fixed.
    static func swallowTransform(canvas: CGSize, target: CGSize, flipped: Bool, anchor: CGPoint) -> CGAffineTransform {
        CGAffineTransform(a: target.width / canvas.width, b: 0, c: 0, d: target.height / canvas.height,
                          tx: -anchor.x * (canvas.width - target.width),
                          ty: (flipped ? -anchor.y : 1 - anchor.y) * (canvas.height - target.height))
    }

    /// Original notch UI starts at the physical screen edge and expands around the camera.
    static func panelFrame(cutout: CGRect, size: CGSize) -> CGRect {
        CGRect(x: cutout.midX - size.width / 2, y: cutout.maxY - size.height,
               width: size.width, height: size.height)
    }

    static func isSaved(in defaults: UserDefaults) -> Bool { defaults.bool(forKey: "notch.docked") }
    static func save(_ docked: Bool, in defaults: UserDefaults) { defaults.set(docked, forKey: "notch.docked") }
}
