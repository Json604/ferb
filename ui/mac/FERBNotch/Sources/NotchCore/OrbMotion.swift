import Foundation
import CoreGraphics

/// A spring in the "Animate with springs" terms: a perceptual duration and a bounce (0 is critically damped).
public struct SpringSpec: Sendable, Equatable {
    public var duration: Double
    public var bounce: Double
    public init(duration: Double, bounce: Double) {
        self.duration = duration
        self.bounce = bounce
    }
    public static let open = SpringSpec(duration: 0.45, bounce: 0.2)
    public static let close = SpringSpec(duration: 0.4, bounce: 0)
}

/// The orb's flight: a mass-1 spring per axis, solved in closed form so a frame step of any size is stable and a
/// critically damped spring (bounce 0) never overshoots.
public struct OrbMotion: Sendable, Equatable {
    /// Settled: within 0.5 pt of the target and below 1 pt/s.
    static let settleDistance: CGFloat = 0.5
    static let settleSpeed: CGFloat = 1

    private var position: CGPoint
    private var target: CGPoint
    private var spec: SpringSpec = .open
    public private(set) var velocity = CGVector.zero

    public init(origin: CGPoint) {
        position = origin
        target = origin
    }

    /// Aims at a new point from where the orb is now; position and velocity carry over.
    public mutating func retarget(_ p: CGPoint, spec: SpringSpec) {
        target = p
        self.spec = spec
    }

    public mutating func step(dt: Double) -> CGPoint {
        guard dt > 0 else { return position }
        let (x, vx) = Self.advance(position.x - target.x, velocity.dx, dt, spec)
        let (y, vy) = Self.advance(position.y - target.y, velocity.dy, dt, spec)
        position = CGPoint(x: target.x + x, y: target.y + y)
        velocity = CGVector(dx: vx, dy: vy)
        return position
    }

    public var settled: Bool {
        hypot(position.x - target.x, position.y - target.y) < Self.settleDistance
            && hypot(velocity.dx, velocity.dy) < Self.settleSpeed
    }

    /// Exact solution of x'' = -k x - c x' over `dt`, for x = distance from the target.
    private static func advance(_ x0: CGFloat, _ v0: CGFloat, _ dt: Double, _ spec: SpringSpec)
        -> (CGFloat, CGFloat) {
        let w = 2 * Double.pi / max(spec.duration, 0.01)          // sqrt(stiffness), mass 1
        let zeta = 1 - min(max(spec.bounce, 0), 0.99)             // damping = 2 zeta w = 4 pi (1 - bounce) / duration
        let x = Double(x0), v = Double(v0)
        let decay = exp(-zeta * w * dt)
        if zeta >= 1 {
            let b = v + w * x
            return (CGFloat((x + b * dt) * decay), CGFloat((v - w * b * dt) * decay))
        }
        let wd = w * (1 - zeta * zeta).squareRoot()
        let a = zeta * w
        let b = (v + a * x) / wd
        let cosine = cos(wd * dt), sine = sin(wd * dt)
        return (CGFloat(decay * (x * cosine + b * sine)),
                CGFloat(decay * (v * cosine - (wd * x + a * b) * sine)))
    }
}

public enum OrbOrigin {
    /// Where the orb leaves from: the camera cutout's centre, else the virtual notch `NotchGeometry` draws.
    /// AppKit coordinates (origin bottom left).
    public static func notchCentre(screenFrame: CGRect, cutout: CGRect?, menuBarHeight: CGFloat = 24) -> CGPoint {
        let pill = cutout ?? NotchGeometry(screen: screenFrame, topInset: 0, leftAreaMaxX: nil,
                                           rightAreaMinX: nil, menuBarHeight: menuBarHeight).cutoutFrame
        return CGPoint(x: pill.midX, y: pill.midY)
    }
}
