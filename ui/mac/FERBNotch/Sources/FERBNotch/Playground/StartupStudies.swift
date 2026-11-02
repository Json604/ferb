#if FERB_PLAYGROUND
import AppKit
import SwiftUI

enum StartupStudy: String, CaseIterable, Identifiable {
    case still = "Still", silk = "Silk", gravity = "Gravity", gravityDense = "Gravity — Dense", gravityTwirl = "Gravity — Twirl", interference = "Interference"
    var id: String { rawValue }
    var title: String { self == .gravity ? "Gravity — Lines" : rawValue }
    var usesGravityTiming: Bool { self == .gravity || self == .gravityDense || self == .gravityTwirl }
    var usesDenseMaterial: Bool { self == .gravityDense || self == .gravityTwirl }
    var note: String {
        switch self {
        case .still: return "The original logo. No added spectacle."
        case .silk: return "Grained ribbons folding through light."
        case .gravity: return "The line-based Gravity, with inward flow and moving highlights."
        case .gravityDense: return "The dense, grainy material version, with the same timing and flow."
        case .gravityTwirl: return "Dense material winding inward, with faster rotation near the logo."
        case .interference: return "Two wave fields forming impossible-looking patterns."
        }
    }
    static var selected: StartupStudy {
        StartupStudy(rawValue: UserDefaults.standard.string(forKey: "concept.arrivalStyle") ?? "") ?? .gravityDense
    }
}

struct StartupStudyField: View {
    let study: StartupStudy
    let time: Double
    let progress: Double
    let energy: Double
    var resonance: [Double] = []

    var body: some View {
        GeometryReader { geometry in
            Canvas(rendersAsynchronously: true) { context, size in
                switch study {
                case .still: break
                case .silk: silk(context: &context, size: size)
                case .gravity: gravity(context: &context, size: size)
                case .gravityDense:
                    DenseGravitySurface(size: size, time: time, progress: progress, energy: energy, resonance: resonance).draw(in: &context)
                case .gravityTwirl:
                    DenseGravitySurface(size: size, time: time, progress: progress, energy: energy, twirls: true).draw(in: &context)
                case .interference: interference(context: &context, size: size)
                }
            }
            .overlay {
                Image(nsImage: StudyGrain.image).resizable(resizingMode: .tile)
                    .frame(width: geometry.size.width, height: geometry.size.height)
                    .blendMode(.softLight).opacity(study.usesDenseMaterial ? 0.23 : 0.32)
            }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    private func silk(context: inout GraphicsContext, size: CGSize) {
        // A folded sheet: adjacent strands share a surface, so highlights travel coherently.
        for strand in 0..<100 {
            let v = Double(strand) / 99
            var path = Path()
            for point in 0...100 {
                let u = Double(point) / 100
                let fold = sin(u * 7.3 + time * 0.46 + v * 3.2)
                let twist = cos(u * 4.4 - time * 0.35 + v * 5.2)
                let x = u * size.width * 1.25 - size.width * 0.125
                let y = size.height * (0.5 + (v - 0.5) * 0.48 * twist + fold * (0.15 + energy * 0.035))
                let p = CGPoint(x: x, y: y)
                if point == 0 { path.move(to: p) } else { path.addLine(to: p) }
            }
            let shimmer = pow(abs(sin(v * .pi * 2 + time * 0.23)), 7)
            context.stroke(path, with: .linearGradient(
                Gradient(colors: [Color(red: 0.2, green: 0.29, blue: 0.36),
                                  Color(white: 0.65 + shimmer * 0.35),
                                  Color(red: 0.62, green: 0.43, blue: 0.29)]),
                startPoint: .zero, endPoint: CGPoint(x: size.width, y: size.height)),
                lineWidth: 0.6 + shimmer * 1.1)
        }
    }

    private func gravity(context: inout GraphicsContext, size: CGSize) {
        GravitySurface(size: size, time: time, progress: progress, energy: energy).draw(in: &context)
    }

    private func interference(context: inout GraphicsContext, size: CGSize) {
        for field in 0..<2 {
            var layer = context
            layer.blendMode = .screen
            let direction = field == 0 ? 1.0 : -1.0
            for strand in 0..<84 {
                let v = Double(strand) / 83
                var path = Path()
                for sample in 0...96 {
                    let u = Double(sample) / 96
                    let phase = u * 6 + direction * time * 0.3
                    let envelope = pow(sin(u * .pi), 0.7)
                    let wave = sin(phase + v * 5) * envelope * 0.24
                    let x = u * size.width
                    let y = size.height * (0.18 + v * 0.64 + direction * wave)
                    let p = CGPoint(x: x, y: y)
                    if sample == 0 { path.move(to: p) } else { path.addLine(to: p) }
                }
                let color = field == 0 ? Color(red: 0.68, green: 0.77, blue: 0.79) : Color(red: 0.64, green: 0.47, blue: 0.37)
                layer.stroke(path, with: .color(color.opacity(0.42 + energy * 0.15)), lineWidth: 0.65)
            }
        }
    }
}

/// One continuous surface with inward travel, perspective and light moving over its folds.
/// Ring seams fade at both depth boundaries; no visible geometry jumps when a ring wraps.
private struct GravitySurface {
    let size: CGSize
    let time: Double
    let progress: Double
    let energy: Double

    private var settle: Double { smooth((progress - 0.61) / 0.15) }
    private var gather: Double { smooth(progress / 0.20) }
    private var travel: Double { time * 0.065 + smooth((progress - 0.32) / 0.30) * 0.13 }
    private var scale: Double { (0.85 + gather * 0.15 + energy * 0.035) * (1 - settle * 0.83) }

    func draw(in context: inout GraphicsContext) {
        let lightAngle = -1.1 + time * 0.24
        for ring in 0..<84 {
            let depth = (Double(ring) / 84 + 2 - travel).truncatingRemainder(dividingBy: 1)
            let edge = smooth(depth / 0.06) * smooth((1 - depth) / 0.13)
            // A traveling compression wave makes the musical crest pass through the material.
            let crest = exp(-pow((depth - (0.94 - progress * 0.9)) * 7, 2)) * energy
            for section in 0..<6 {
                var path = Path()
                for sample in 0...24 {
                    let angle = (Double(section) + Double(sample) / 24) * .pi / 3
                    let point = projected(depth: depth, angle: angle)
                    if sample == 0 { path.move(to: point) } else { path.addLine(to: point) }
                }
                func light(at angle: Double) -> Color {
                    let specular = pow(max(0, cos(angle - lightAngle - depth * 1.2)), 7)
                    let counterlight = pow(max(0, cos(angle - lightAngle - depth * 1.2 + .pi)), 5)
                    let alpha = edge * (0.25 + specular * 0.48 + counterlight * 0.28 + crest * 0.30)
                        * (1 - settle * 0.6)
                    let warm = max(0, sin(angle + time * 0.16))
                    return Color(red: 0.73 + warm * 0.25, green: 0.79 - warm * 0.16,
                                 blue: 0.83 - warm * 0.39).opacity(alpha)
                }
                let startAngle = Double(section) * .pi / 3
                let endAngle = Double(section + 1) * .pi / 3
                context.stroke(path, with: .linearGradient(
                    Gradient(colors: [light(at: startAngle), light(at: endAngle)]),
                    startPoint: projected(depth: depth, angle: startAngle),
                    endPoint: projected(depth: depth, angle: endAngle)), lineWidth: 0.65 + crest * 0.6)
            }
        }
        // A handful of luminous filaments follow the same surface, rather than orbiting as decoration.
        for index in 0..<7 {
            let depth = 0.22 + Double(index) * 0.085
            let angle = time * (0.20 + Double(index) * 0.016) + Double(index) * 2.399
            var filament = Path()
            for sample in 0...30 {
                let portion = Double(sample) / 30
                let point = projected(depth: depth + portion * 0.035, angle: angle + portion * 0.54)
                if sample == 0 { filament.move(to: point) } else { filament.addLine(to: point) }
            }
            let start = projected(depth: depth, angle: angle)
            let end = projected(depth: depth + 0.035, angle: angle + 0.54)
            context.stroke(filament, with: .linearGradient(
                Gradient(colors: [.clear, Color(red: 1, green: 0.87, blue: 0.68).opacity(0.20 + energy * 0.35)]),
                startPoint: start, endPoint: end), lineWidth: 0.85)
        }
        // Sparse motes carry parallax at different depths, without a random flicker each frame.
        for index in 0..<48 {
            let depth = (Double(index) * 0.618033 + 2 - travel * 0.8).truncatingRemainder(dividingBy: 1)
            let angle = Double(index) * 2.399 + time * 0.1
            let p = projected(depth: depth, angle: angle)
            let opacity = sin(depth * .pi) * (0.14 + energy * 0.38) * (1 - settle)
            let diameter = 0.8 + depth
            context.fill(Path(ellipseIn: CGRect(x: p.x, y: p.y, width: diameter, height: diameter)),
                         with: .color(.white.opacity(opacity)))
        }
    }

    private func projected(depth: Double, angle: Double) -> CGPoint {
        let base = min(size.width, size.height)
        let radius = base * (0.080 + pow(depth, 1.55) * 0.84) * scale
        let fold = 1 + 0.07 * sin(angle * 3 - depth * 7 + time * 0.65)
            + 0.025 * sin(angle * 5 + depth * 11 - time * 0.4)
        let twist = depth * 1.35 + time * 0.12
        let inclination = 0.88 + 0.12 * sin(time * 0.55)
        let x = cos(angle + twist) * radius * fold
        let y = sin(angle + twist) * radius * fold
        let z = y * sin(inclination)
        let perspective = base * 1.8 / (base * 1.8 + z)
        let projectedY = y * cos(inclination)
        let roll = -0.22 + sin(time * 0.40) * 0.12
        let centerX = sin(depth * 4 + time * 0.3) * depth * 24 * scale
        let centerY = cos(depth * 3 - time * 0.3) * depth * 12 * scale
        return CGPoint(x: size.width / 2 + (x * cos(roll) - projectedY * sin(roll)) * perspective + centerX,
                       y: size.height / 2 + (x * sin(roll) + projectedY * cos(roll)) * perspective + centerY)
    }

    private func smooth(_ value: Double) -> Double {
        let t = min(1, max(0, value))
        return t * t * (3 - 2 * t)
    }
}

/// Fixed film grain, generated once. It doesn't flicker or allocate random dots every frame.
private enum StudyGrain {
    static let image: NSImage = {
        let side = 256
        guard let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: side, pixelsHigh: side,
                                            bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true,
                                            isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: side * 4,
                                            bitsPerPixel: 32), let bytes = bitmap.bitmapData else { return NSImage() }
        var seed: UInt32 = 604
        for i in 0..<(side * side) {
            seed = seed &* 1664525 &+ 1013904223
            let value = UInt8(truncatingIfNeeded: seed >> 24)
            bytes[i * 4] = value; bytes[i * 4 + 1] = value; bytes[i * 4 + 2] = value; bytes[i * 4 + 3] = 255
        }
        let image = NSImage(size: NSSize(width: side, height: side))
        image.addRepresentation(bitmap)
        return image
    }()
}
#endif
