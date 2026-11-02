#if FERB_PLAYGROUND
import SwiftUI

/// Dense material with a fade-only ending; the optional twirl retains its differential inward motion.
struct DenseGravitySurface {
    let size: CGSize
    let time: Double
    let progress: Double
    let energy: Double
    var twirls = false
    var resonance: [Double] = []

    private var settle: Double { smooth((progress - 0.61) / 0.15) }
    private var gather: Double { smooth(progress / 0.20) }
    private var travel: Double { time * 0.065 + smooth((progress - 0.32) / 0.30) * 0.13 }
    private var scale: Double {
        0.85 + gather * 0.15 + energy * 0.035
    }

    func draw(in context: inout GraphicsContext) {
        let lightAngle = -1.1 + time * 0.24
        let bands = 256
        let offset = (1 - (travel * Double(bands)).truncatingRemainder(dividingBy: 1)) / Double(bands)
        let depths = (0...bands).map { min(1, Double($0) / Double(bands) + offset) }
        let boundaries = depths.map { projectedBoundary(depth: $0) }
        for band in 0..<bands {
            let depth = depths[band]
            let edge = smooth(depth / 0.06) * smooth((1 - depth) / 0.13)
            let crest = exp(-pow((depth - (0.94 - progress * 0.9)) * 7, 2)) * energy
            for section in 0..<6 {
                // Neighboring ribbons share exactly the same boundary; geometry is computed once.
                var ribbon = Path()
                for sample in 0...24 {
                    let point = boundaries[band][section * 24 + sample]
                    if sample == 0 { ribbon.move(to: point) } else { ribbon.addLine(to: point) }
                }
                for sample in (0...24).reversed() {
                    ribbon.addLine(to: boundaries[band + 1][section * 24 + sample])
                }
                ribbon.closeSubpath()
                func light(at angle: Double) -> Color {
                    let specular = pow(max(0, cos(angle - lightAngle - depth * 1.2)), 7)
                    let counterlight = pow(max(0, cos(angle - lightAngle - depth * 1.2 + .pi)), 5)
                    let fold: Double
                    if twirls {
                        // Three material folds remain attached to the differentially rotating surface.
                        let arm = (1 + sin(angle * 3 + depth * 8 - time * 0.18)) / 2
                        fold = 0.40 + 0.60 * pow(arm, 1.4)
                    } else {
                        fold = 0.78 + 0.22 * sin(depth * 17 + angle * 2 - time * 0.3)
                    }
                    let fiber = 0.94 + 0.06 * sin(depth * 440 + angle * 3)
                    let alpha = edge * (0.12 + specular * 0.42 + counterlight * 0.22 + crest * 0.24)
                        * fold * fiber * (1 - settle * 0.6)
                    let warm = max(0, sin(angle + time * 0.16))
                    return Color(red: 0.73 + warm * 0.25, green: 0.79 - warm * 0.16,
                                 blue: 0.83 - warm * 0.39).opacity(alpha)
                }
                let startAngle = Double(section) * .pi / 3
                let endAngle = Double(section + 1) * .pi / 3
                context.fill(ribbon, with: .linearGradient(
                    Gradient(colors: [light(at: startAngle), light(at: endAngle)]),
                    startPoint: boundaries[band][section * 24],
                    endPoint: boundaries[band][(section + 1) * 24]), style: FillStyle(antialiased: false))
            }
        }
    }

    /// Angular terms are fixed. Each ring computes its depth terms once, then projects shared vertices.
    private struct Direction {
        let sine: Double
        let cosine: Double
        let sine3: Double
        let cosine3: Double
        let sine5: Double
        let cosine5: Double
    }
    private static let directions: [Direction] = (0...144).map { sample in
        let angle = Double(sample) / 144 * .pi * 2
        return Direction(sine: sin(angle), cosine: cos(angle), sine3: sin(angle * 3),
                         cosine3: cos(angle * 3), sine5: sin(angle * 5), cosine5: cos(angle * 5))
    }

    private func projectedBoundary(depth: Double) -> [CGPoint] {
        let base = min(size.width, size.height)
        let fieldScale = scale
        // The inner material spirals into the mark first; the whole field never scales down in twirl mode.
        let inwardPull = twirls ? 1 / (1 + settle * (0.5 + 4 * pow(1 - depth, 1.2))) : 1
        // Dense extends underneath the mark instead of leaving a large empty annulus around it.
        let core = twirls ? 0.080 : 0.018
        let pulse = response(at: depth)
        let radialResponse = 1 + pulse * 0.055 * sin(depth * .pi)
        let radius = base * (core + pow(depth, 1.55) * (0.92 - core)) * fieldScale * inwardPull * radialResponse
        let phase3 = -depth * 7 + time * 0.65 + pulse * 0.40
        let phase5 = depth * 11 - time * 0.4 + pulse * 0.20
        let sin3 = sin(phase3), cos3 = cos(phase3), sin5 = sin(phase5), cos5 = cos(phase5)
        let twist: Double
        if twirls {
            let inner = pow(1 - depth, 1.4)
            twist = depth * 1.35 + time * (0.32 + 0.85 * inner) + settle * inner * 1.2
        } else {
            twist = depth * 1.35 + time * 0.12
        }
        let sinTwist = sin(twist), cosTwist = cos(twist)
        let inclination = 0.88 + 0.12 * sin(time * 0.55)
        let sinInclination = sin(inclination), cosInclination = cos(inclination)
        let roll = -0.22 + sin(time * 0.40) * 0.12
        let sinRoll = sin(roll), cosRoll = cos(roll)
        let centerX = size.width / 2 + sin(depth * 4 + time * 0.3) * depth * 24 * fieldScale
        let centerY = size.height / 2 + cos(depth * 3 - time * 0.3) * depth * 12 * fieldScale
        return Self.directions.map { direction in
            let fold = 1 + 0.07 * (direction.sine3 * cos3 + direction.cosine3 * sin3)
                + 0.025 * (direction.sine5 * cos5 + direction.cosine5 * sin5)
            let x = (direction.cosine * cosTwist - direction.sine * sinTwist) * radius * fold
            let y = (direction.sine * cosTwist + direction.cosine * sinTwist) * radius * fold
            let perspective = base * 1.8 / (base * 1.8 + y * sinInclination)
            let projectedY = y * cosInclination
            return CGPoint(x: centerX + (x * cosRoll - projectedY * sinRoll) * perspective,
                           y: centerY + (x * sinRoll + projectedY * cosRoll) * perspective)
        }
    }

    private func response(at depth: Double) -> Double {
        guard !resonance.isEmpty else { return 0 }
        let position = min(1, max(0, depth)) * Double(resonance.count - 1)
        let lower = Int(position), upper = min(resonance.count - 1, lower + 1)
        return resonance[lower] + (resonance[upper] - resonance[lower]) * (position - Double(lower))
    }

    private func smooth(_ value: Double) -> Double {
        let t = min(1, max(0, value))
        return t * t * (3 - 2 * t)
    }
}

#endif
