import Foundation
import CoreGraphics

/// One deterministic clock for physical placement, edge tension, and sound.
/// Geometry is in AppKit screen coordinates; influence always uses the raw grab.
struct MagneticDocking {
    enum Phase { case idle, dragging, fusing, docked }
    struct Sample {
        var time: TimeInterval = 0
        var phase: Phase = .idle
        var rawPill = CGRect.zero
        var pill = CGRect.zero
        var target: CGRect?
        var proximity: CGFloat = 0
        var direction = CGPoint.zero
        var offset = CGPoint.zero
        var velocity = CGPoint.zero
        var strain: CGFloat = 0
        var fusionProgress: CGFloat = 0
        var fusionTranslation = CGPoint.zero
        var completed = false
        var reduceMotion = false

        func fusionRect(from start: CGRect, to target: CGRect) -> CGRect {
            LiveDragLayout.interpolate(start, target, progress: fusionProgress)
                .offsetBy(dx: fusionTranslation.x, dy: fusionTranslation.y)
        }
    }
    private(set) var current = Sample()
    private var armed = true
    private var lastTime: TimeInterval = 0
    private var fusionStart = CGRect.zero
    private var fusionTarget = CGRect.zero
    private var fusionVelocity = CGPoint.zero
    private var fusionTime: TimeInterval = 0
    private var pointerVelocity = CGPoint.zero
    static let influenceRadius: CGFloat = 150
    static let fusionDuration: TimeInterval = 0.22

    mutating func begin(at time: TimeInterval, tornOut: Bool = false) {
        // A re-grab adopts the visible frame; remaining velocity continues as
        // physical offset rather than changing the owner's new pointer anchor.
        let velocity = current.velocity
        current = Sample(time: time, phase: .dragging, velocity: velocity)
        lastTime = time
        armed = !tornOut
    }

    mutating func update(pill: CGRect, target: CGRect?, at time: TimeInterval,
                         reduceMotion: Bool = false, pointerVelocity: CGPoint = .zero) -> Sample {
        guard current.phase == .dragging else { return sample(at: time, reduceMotion: reduceMotion) }
        let dt = max(0, time - lastTime)
        lastTime = max(time, lastTime)
        current.rawPill = pill
        current.target = target
        self.pointerVelocity = pointerVelocity
        let dx = target.map { max($0.minX - pill.maxX, max(pill.minX - $0.maxX, 0)) } ?? .infinity
        let dy = target.map { max($0.minY - pill.maxY, max(pill.minY - $0.maxY, 0)) } ?? .infinity
        let distance = hypot(dx, dy)
        if distance >= Self.influenceRadius { armed = true }
        let unit = armed ? max(0, 1 - distance / Self.influenceRadius) : 0
        current.proximity = unit * unit * (3 - 2 * unit)
        let toward = target.map { CGPoint(x: min($0.maxX, max($0.minX, pill.midX)) - pill.midX,
                                             y: min($0.maxY, max($0.minY, pill.midY)) - pill.midY) } ?? .zero
        let length = hypot(toward.x, toward.y)
        current.direction = length > 0 ? CGPoint(x: toward.x / length, y: toward.y / length) : .zero
        let strength = 10 * current.proximity * current.proximity
        let desired = CGPoint(x: current.direction.x * strength, y: current.direction.y * strength)
        if reduceMotion {
            current.offset = .zero
            current.velocity = .zero
        } else {
            // Exact critically damped integration stays stable after stalled frames.
            func step(_ x: CGFloat, _ v: CGFloat, _ target: CGFloat) -> (CGFloat, CGFloat) {
                let omega: CGFloat = 24
                let t = CGFloat(dt), delta = x - target, c = v + omega * delta
                let decay = exp(-omega * t)
                return (target + (delta + c * t) * decay, (v - omega * c * t) * decay)
            }
            let x = step(current.offset.x, current.velocity.x, desired.x)
            let y = step(current.offset.y, current.velocity.y, desired.y)
            current.offset = CGPoint(x: x.0, y: y.0)
            current.velocity = CGPoint(x: x.1, y: y.1)
        }
        current.pill = pill.offsetBy(dx: current.offset.x, dy: current.offset.y)
        current.strain = reduceMotion || !armed ? 0 : 0.018 * pow(max(0, 1 - distance / 40), 2)
        current.time = time
        current.reduceMotion = reduceMotion
        current.completed = false
        return current
    }

    mutating func release(to target: CGRect, at time: TimeInterval) {
        fusionStart = current.pill
        fusionVelocity = CGPoint(x: current.velocity.x + pointerVelocity.x, y: current.velocity.y + pointerVelocity.y)
        current.velocity = fusionVelocity
        fusionTime = time
        fusionTarget = target
        current.phase = .fusing
        current.completed = false
    }

    mutating func sample(at time: TimeInterval, reduceMotion: Bool = false) -> Sample {
        current.completed = false
        current.time = time
        current.reduceMotion = reduceMotion
        guard current.phase == .fusing else { return current }
        let target = fusionTarget
        let elapsed = max(0, time - fusionTime)
        let u = min(1, CGFloat(elapsed / Self.fusionDuration))
        // Critically damped attraction with a tiny endpoint correction so both
        // position and velocity reach the exact join on the final display frame.
        let w: CGFloat = 9
        let residual = (1 + w) * exp(-w)
        let terminalSlope = w * w * exp(-w)
        let correction = residual * u * u * (3 - 2 * u) - terminalSlope * (u*u*u-u*u)
        let progress = reduceMotion ? 1 : 1 - (1 + w * u) * exp(-w * u) + correction
        current.fusionProgress = progress
        current.pill = LiveDragLayout.interpolate(fusionStart, target, progress: progress)
        let carry = reduceMotion ? 0 : CGFloat(Self.fusionDuration) * u * pow(1 - u, 3)
        current.fusionTranslation = CGPoint(x: fusionVelocity.x * carry, y: fusionVelocity.y * carry)
        current.pill = current.pill.offsetBy(dx: current.fusionTranslation.x, dy: current.fusionTranslation.y)
        let derivative = (w * w * u * exp(-w * u) + residual * 6 * u * (1-u) - terminalSlope * (3*u*u-2*u)) / CGFloat(Self.fusionDuration)
        let carryDerivative = pow(1-u, 2) * (1-4*u)
        current.velocity = CGPoint(x: (target.minX-fusionStart.minX)*derivative + fusionVelocity.x*carryDerivative,
                                   y: (target.minY-fusionStart.minY)*derivative + fusionVelocity.y*carryDerivative)
        current.proximity = 1
        current.strain = reduceMotion ? 0 : 0.018 * (1 - progress)
        if u >= 1 || reduceMotion {
            current.pill = target
            current.velocity = .zero
            current.offset = .zero
            current.phase = .docked
            current.fusionProgress = 1
            current.completed = true
        }
        return current
    }

    mutating func cancel(at time: TimeInterval) {
        current = Sample(time: time)
        lastTime = time
    }
}
