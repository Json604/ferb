import AppKit

/// Screen-coordinate geometry shared by the real capsule and its edge response.
enum MagneticFieldGeometry {
    static func strainedPill(_ sample: MagneticDocking.Sample) -> CGRect {
        guard !sample.reduceMotion else { return sample.pill }
        let dx = sample.pill.width * sample.strain * abs(sample.direction.x)
        let dy = sample.pill.height * sample.strain * abs(sample.direction.y)
        return CGRect(x: sample.pill.minX - (sample.direction.x < 0 ? dx : 0),
                      y: sample.pill.minY - (sample.direction.y < 0 ? dy : 0),
                      width: sample.pill.width + dx, height: sample.pill.height + dy)
    }
}

/// Noninteractive native layers, with all energy restricted to facing surfaces.
/// The hardware cutout itself is never painted.
final class MagneticFieldView: NSView {
    private var phase: Double = 0
    private var lastTime: TimeInterval?
    private let pillRipples = (0..<3).map { _ in CAShapeLayer() }
    private let notchRipples = (0..<3).map { _ in CAShapeLayer() }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        for ripple in pillRipples + notchRipples {
            ripple.fillColor = nil
            ripple.strokeColor = NSColor(calibratedRed: 0.43, green: 0.57, blue: 0.61, alpha: 1).cgColor
            ripple.lineWidth = 0.8
            ripple.lineCap = .round
            ripple.shadowColor = ripple.strokeColor
            ripple.shadowRadius = 2
            ripple.shadowOpacity = 0.35
            layer?.addSublayer(ripple)
        }
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func update(_ sample: MagneticDocking.Sample, canvas: CGRect) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        defer { CATransaction.commit() }
        let elapsed = lastTime.map { max(0, sample.time - $0) } ?? 0
        lastTime = sample.time
        let collapse = 1 - sample.fusionProgress
        let intensity = sample.proximity * collapse
        guard let target = sample.target, intensity > 0 else {
            for ripple in pillRipples + notchRipples { ripple.opacity = 0 }
            return
        }
        let pill = MagneticFieldGeometry.strainedPill(sample).offsetBy(dx: -canvas.minX, dy: -canvas.minY)
        let notch = target.offsetBy(dx: -canvas.minX, dy: -canvas.minY)
        let direction = sample.direction
        if !sample.reduceMotion {
            phase += elapsed * (3 + 7*Double(sample.proximity))
            phase.formTruncatingRemainder(dividingBy: 2 * .pi)
        }
        let phase = sample.reduceMotion ? 0 : phase
        for index in 0..<3 {
            let spacing = CGFloat(index) * (2.8 - 1.8*sample.proximity) * collapse
            let wave = sample.reduceMotion ? 0 : sin(phase + Double(index)*0.8) * 0.65 * intensity
            let path = CGMutablePath()
            // Follow a continuous arc of the rounded capsule, rather than
            // switching abruptly between top and side at a diagonal approach.
            let centerAngle = atan2(direction.y, direction.x)
            for step in 0...20 {
                let fraction = CGFloat(step)/20
                let angle = centerAngle + (fraction - 0.5)*1.5
                let dx = cos(angle), dy = sin(angle)
                let halfWidth = pill.width/2, halfHeight = pill.height/2
                let radius = min(15, halfHeight, halfWidth)
                var distance = min(halfWidth/max(abs(dx), 0.000001), halfHeight/max(abs(dy), 0.000001))
                if abs(dx*distance) > halfWidth-radius, abs(dy*distance) > halfHeight-radius {
                    let cx = (dx >= 0 ? 1 : -1)*(halfWidth-radius)
                    let cy = (dy >= 0 ? 1 : -1)*(halfHeight-radius)
                    let dot = dx*cx + dy*cy
                    distance = dot + sqrt(max(0, dot*dot - cx*cx - cy*cy + radius*radius))
                }
                distance += spacing - 0.7 + wave*sin(fraction * .pi)
                let point = CGPoint(x: pill.midX + dx*distance, y: pill.midY + dy*distance)
                if step == 0 { path.move(to: point) } else { path.addLine(to: point) }
            }
            pillRipples[index].path = path
            pillRipples[index].opacity = Float(intensity * (index == 0 ? 0.28 : 0.11))
            let answer = CGMutablePath()
            let center = min(notch.maxX-14, max(notch.minX+14, pill.midX))
            let width: CGFloat = 13 + 12*intensity
            let y = notch.minY - 1 - spacing
            answer.move(to: CGPoint(x: center-width, y: y))
            answer.addQuadCurve(to: CGPoint(x: center+width, y: y), control: CGPoint(x: center, y: y-wave))
            notchRipples[index].path = answer
            notchRipples[index].opacity = pillRipples[index].opacity
        }
    }
}
