import AppKit
import SwiftUI
import NotchCore

enum Palette {
    static let ink = Color(nsColor: .labelColor)
    static let secondary = Color(nsColor: .secondaryLabelColor)
    static let green = Color(.sRGB, red: 0.05, green: 1, blue: 0.24, opacity: 1)
    static let amber = Color(.sRGB, red: 1, green: 0.65, blue: 0.06, opacity: 1)
    static let red = Color(.sRGB, red: 1, green: 0.14, blue: 0.19, opacity: 1)

    static func taskColor(_ phase: SessionPhase) -> Color {
        switch phase {
        case .working: return amber
        case .attention: return red
        case .result: return green
        default: return secondary
        }
    }
}

/// Concave shoulders join the screen edge; the lower corners echo the camera cutout.
struct NotchSurfaceShape: Shape {
    var expansion: CGFloat = 1
    var docked = false
    var joining: CGFloat? = nil
    var animatableData: CGFloat {
        get { expansion }
        set { expansion = newValue }
    }

    func path(in rect: CGRect) -> Path {
        if let joining, joining < 1 {
            let t = max(0, joining)
            let radius = min(rect.height / 2, 18 * (0.75 + 0.25 * expansion))
            let shoulder = 10 * expansion * t
            let top = radius * (1-t)
            let bottom = radius * (1-t) + (12+7*expansion)*t
            var path = Path()
            path.move(to: CGPoint(x: rect.minX+top, y: rect.minY))
            path.addLine(to: CGPoint(x: rect.maxX-top, y: rect.minY))
            path.addQuadCurve(to: CGPoint(x: rect.maxX-shoulder, y: rect.minY+top+shoulder),
                              control: CGPoint(x: rect.maxX-shoulder, y: rect.minY))
            path.addLine(to: CGPoint(x: rect.maxX-shoulder, y: rect.maxY-bottom))
            path.addQuadCurve(to: CGPoint(x: rect.maxX-shoulder-bottom, y: rect.maxY), control: CGPoint(x: rect.maxX-shoulder, y: rect.maxY))
            path.addLine(to: CGPoint(x: rect.minX+shoulder+bottom, y: rect.maxY))
            path.addQuadCurve(to: CGPoint(x: rect.minX+shoulder, y: rect.maxY-bottom), control: CGPoint(x: rect.minX+shoulder, y: rect.maxY))
            path.addLine(to: CGPoint(x: rect.minX+shoulder, y: rect.minY+top+shoulder))
            path.addQuadCurve(to: CGPoint(x: rect.minX+top, y: rect.minY), control: CGPoint(x: rect.minX+shoulder, y: rect.minY))
            path.closeSubpath()
            return path
        }
        guard docked || joining == 1 else {
            let radius = min(rect.height / 2, 18 * (0.75 + 0.25 * expansion))
            return RoundedRectangle(cornerRadius: radius, style: .continuous).path(in: rect)
        }
        // Original FERB notch silhouette: concave shoulders meet the screen edge.
        let shoulder: CGFloat = 10 * expansion
        let bottom: CGFloat = 12 + 7 * expansion
        var path = Path()
        path.move(to: CGPoint(x: rect.minX, y: rect.minY))
        path.addLine(to: CGPoint(x: rect.maxX, y: rect.minY))
        path.addQuadCurve(to: CGPoint(x: rect.maxX - shoulder, y: rect.minY + shoulder),
                          control: CGPoint(x: rect.maxX - shoulder, y: rect.minY))
        path.addLine(to: CGPoint(x: rect.maxX - shoulder, y: rect.maxY - bottom))
        path.addQuadCurve(to: CGPoint(x: rect.maxX - shoulder - bottom, y: rect.maxY),
                          control: CGPoint(x: rect.maxX - shoulder, y: rect.maxY))
        path.addLine(to: CGPoint(x: rect.minX + shoulder + bottom, y: rect.maxY))
        path.addQuadCurve(to: CGPoint(x: rect.minX + shoulder, y: rect.maxY - bottom),
                          control: CGPoint(x: rect.minX + shoulder, y: rect.maxY))
        path.addLine(to: CGPoint(x: rect.minX + shoulder, y: rect.minY + shoulder))
        path.addQuadCurve(to: CGPoint(x: rect.minX, y: rect.minY),
                          control: CGPoint(x: rect.minX + shoulder, y: rect.minY))
        path.closeSubpath()
        return path
    }
}

struct FerbButtonStyle: ButtonStyle {
    var primary = false
    var quiet = false
    @State private var hovered = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.isEnabled) private var enabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 12, weight: .medium))
            .foregroundStyle(Palette.ink)
            .padding(.horizontal, quiet ? 7 : 10)
            .frame(minHeight: 28)
            .background {
                RoundedRectangle(cornerRadius: 9, style: .continuous)
                    .fill(.white.opacity((primary ? 0.085 : (quiet ? 0 : 0.045)) + (hovered ? 0.045 : 0)))
            }
            .opacity(enabled ? (configuration.isPressed ? 0.65 : 1) : 0.35)
            .scaleEffect(configuration.isPressed && !reduceMotion ? 0.97 : 1)
            .animation(reduceMotion ? .easeOut(duration: 0.1) : .spring(duration: 0.18, bounce: 0.08), value: configuration.isPressed)
            .animation(.easeOut(duration: 0.12), value: hovered)
            .onHover { hovered = $0 }
            .contentShape(RoundedRectangle(cornerRadius: 9))
    }
}

/// An example signal, separate from the future microphone meter.
struct VoiceLight: View {
    let listening: Bool
    let startedAt: Date
    var active = true
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 60, paused: !active || reduceMotion)) { timeline in
            let elapsed = timeline.date.timeIntervalSince(startedAt)
            HStack(alignment: .center, spacing: 2) {
                ForEach(0..<49, id: \.self) { index in
                    Capsule()
                        .fill(.white.opacity(listening ? 0.94 : 0.64))
                        .frame(width: 2, height: height(index, elapsed: elapsed))
                }
            }
            .shadow(color: .white.opacity(listening ? 0.3 : 0.12), radius: 4)
            .frame(height: 27)
        }
        .accessibilityHidden(true)
    }

    private func height(_ index: Int, elapsed: Double) -> CGFloat {
        let x = Double(index)
        let envelope = 3 + 16 * exp(-pow((x - 15) / 5, 2)) + 22 * exp(-pow((x - 30) / 6, 2))
        if reduceMotion { return listening ? 5 : max(2, envelope * 0.25) }
        // A low-amplitude breath while visible; it is not a microphone signal.
        guard listening else { return max(2, envelope * (0.23 + 0.045 * sin(elapsed * 1.5 + x * 0.09))) }
        let movement = 0.27 + 0.73 * abs(sin(elapsed * 4.6 + x * 0.18) * cos(elapsed * 2.1 - x * 0.09))
        return max(2, envelope * movement)
    }
}

struct SeamShape: Shape {
    func path(in rect: CGRect) -> Path {
        var path = Path()
        let radius: CGFloat = min(12, rect.height)
        path.move(to: .zero)
        path.addLine(to: CGPoint(x: rect.maxX, y: 0))
        path.addLine(to: CGPoint(x: rect.maxX, y: rect.maxY - radius))
        path.addQuadCurve(to: CGPoint(x: rect.maxX - radius, y: rect.maxY),
                          control: CGPoint(x: rect.maxX, y: rect.maxY))
        path.addLine(to: CGPoint(x: radius, y: rect.maxY))
        path.addQuadCurve(to: CGPoint(x: 0, y: rect.maxY - radius), control: CGPoint(x: 0, y: rect.maxY))
        path.closeSubpath()
        return path
    }
}

struct SectionCaption: View {
    let text: String
    var body: some View {
        Text(text)
            .font(.system(size: 12, weight: .medium))
            .foregroundStyle(Palette.secondary)
    }
}
