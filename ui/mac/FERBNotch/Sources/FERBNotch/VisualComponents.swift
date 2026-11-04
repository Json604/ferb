import AppKit
import SwiftUI
import NotchCore

enum Palette {
    static let ink = Color(nsColor: .labelColor)
    static let secondary = Color(nsColor: .secondaryLabelColor)
    static let green = Color(.sRGB, red: 0.05, green: 1, blue: 0.24, opacity: 1)
    static let amber = Color(.sRGB, red: 1, green: 0.65, blue: 0.06, opacity: 1)
    static let red = Color(.sRGB, red: 1, green: 0.14, blue: 0.19, opacity: 1)
    /// Answers in steps (APP-300, the approved mock's colours): a failed row's words, a paused row, an updated row.
    static let redSoft = Color(.sRGB, red: 1, green: 0.48, blue: 0.5, opacity: 1)
    static let chatBlue = Color.accentColor

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

/// The dictation meter (APP-674): drawn only from the microphone's loudness (VoiceLevel), never from a clock. The
/// newest level sits in the centre and older ones move outward, so the bars are a still line in silence and move
/// only with the voice. Reduce Motion keeps the bars still and shows loudness by brightness alone.
struct VoiceLight: View {
    @ObservedObject var meter: VoiceMeter
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    static let bars = 49
    static let quietHeight: CGFloat = 2, tallest: CGFloat = 27

    var body: some View {
        let value = meter.value
        HStack(alignment: .center, spacing: 2) {
            ForEach(0..<Self.bars, id: \.self) { index in
                Capsule()
                    .fill(.white.opacity(reduceMotion ? 0.4 + 0.54 * value.level : 0.94))
                    .frame(width: 2, height: reduceMotion ? 3 : Self.height(index, history: value.history))
            }
        }
        // Blocks can arrive in bursts (a 100 ms hardware buffer is five of them): a short linear blend, no spring.
        .animation(reduceMotion ? nil : .linear(duration: 0.08), value: value.history)
        .shadow(color: .white.opacity(0.3), radius: 4)
        .frame(height: Self.tallest)
        .accessibilityHidden(true)
    }

    /// A bar's height: the level `distance` blocks old, tapered toward the edges; the quiet height at level 0.
    static func height(_ index: Int, history: [Double]) -> CGFloat {
        let distance = abs(index - (bars - 1) / 2)
        let level = distance < history.count ? history[distance] : 0
        let taper = 0.35 + 0.65 * exp(-pow(Double(distance) / 12, 2))
        return quietHeight + (tallest - quietHeight) * CGFloat(taper * min(1, max(0, level)))
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
