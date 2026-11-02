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

    /// Original notch UI starts at the physical screen edge and expands around the camera.
    static func panelFrame(cutout: CGRect, size: CGSize) -> CGRect {
        CGRect(x: cutout.midX - size.width / 2, y: cutout.maxY - size.height,
               width: size.width, height: size.height)
    }

    /// Docked in the camera housing: as the owner last left it, and on a first run (nothing saved) whenever the screen
    /// has a hardware notch (APP-550). Without one there is nothing to dock into: the handle sits on an edge.
    static func isDocked(in defaults: UserDefaults, hasNotch: Bool) -> Bool {
        hasNotch && (defaults.object(forKey: "notch.docked") as? Bool ?? true)
    }
    static func save(_ docked: Bool, in defaults: UserDefaults) { defaults.set(docked, forKey: "notch.docked") }

    /// The edge the handle sits on when undocked: where the owner dropped it, else the top (on a notchless Mac the
    /// top centre, where the virtual notch is).
    static func edge(in defaults: UserDefaults) -> FERBScreenEdge {
        defaults.string(forKey: "notch.indicatorEdge").flatMap(FERBScreenEdge.init(rawValue:)) ?? .top
    }

    /// Where along that edge, 0…1: the middle unless the owner dropped it elsewhere.
    static func fraction(in defaults: UserDefaults) -> CGFloat {
        defaults.object(forKey: "notch.indicatorFraction") == nil
            ? 0.5 : min(1, max(0, CGFloat(defaults.double(forKey: "notch.indicatorFraction"))))
    }
}
