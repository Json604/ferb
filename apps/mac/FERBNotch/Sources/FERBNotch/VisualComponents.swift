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
    /// The same colours for chat text drawn by FERB's own text view (NativeMessageText, APP-788).
    static let redSoftText = NSColor(redSoft)
    static let amberText = NSColor(amber)
    static let greenText = NSColor(green)

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
                    .fill(.white.opacity(HoverLook.buttonFill(primary: primary, quiet: quiet, hovered: hovered && enabled)))
            }
            .opacity(enabled ? (configuration.isPressed ? 0.65 : 1) : 0.35)
            .scaleEffect(configuration.isPressed && !reduceMotion ? 0.97 : 1)
            .animation(reduceMotion ? .easeOut(duration: 0.1) : .spring(duration: 0.18, bounce: 0.08), value: configuration.isPressed)
            // APP-779: the AppKit hover source (APP-753): SwiftUI's onHover never fires in FERB's non-activating panels,
            // so the hover colour and the pointing hand come from it; clicks pass through it to the button
            .overlay {
                HoverArea(pointing: enabled, label: "button") { inside in
                    withAnimation(HoverLook.duration(reduceMotion: reduceMotion).map { .easeOut(duration: $0) }) {
                        hovered = inside
                    }
                }
            }
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

/// APP-889 (owner 2026-11-11: "the image enlarged but theres no lcose button"): a picture a click shows larger, in a
/// popover hidden from screen sharing (APP-406), the same way everywhere it is used (a payment page, a row's live
/// preview). Closing is plain and standard: its ✕, a click on the larger picture, a click outside it, or Esc
/// (NotchModel.closeEnlargedPicture, which the notch and the chat window ask first); the keyboard focus then goes back
/// where it was.
struct EnlargeablePicture<Small: View, Large: View>: View {
    @ObservedObject var model: NotchModel
    let label: String
    @ViewBuilder let small: () -> Small
    @ViewBuilder let large: () -> Large
    /// This view's own key: the same picture drawn in two places never opens twice.
    @State private var key = UUID().uuidString

    var body: some View {
        let model = model, key = key
        Button { model.togglePicture(key) } label: { small() }
            .buttonStyle(.plain)
            .help(model.isEnlarged(key) ? "Click to make it small again" : "Click to enlarge")
            .accessibilityLabel(label)
            .accessibilityHint("Shows it larger")
            .popover(isPresented: Binding(get: { model.isEnlarged(key) },
                                         set: { if !$0, model.isEnlarged(key) { model.closeEnlargedPicture() } }),
                     arrowEdge: .bottom) {
                EnlargedPicture(label: label, close: { model.closeEnlargedPicture() }, content: large)
                    .background(TaskPreviewWindow(model: model, hidesFromCapture: true))
            }
    }
}

/// The larger picture with its ✕ in the top corner; a click on the picture makes it small again, as does Esc.
struct EnlargedPicture<Content: View>: View {
    let label: String
    let close: () -> Void
    @ViewBuilder let content: () -> Content

    var body: some View {
        content()
            .contentShape(Rectangle())
            .onTapGesture { close() }
            .overlay(alignment: .topTrailing) {
                Button(action: close) {
                    Image(systemName: "xmark").font(.system(size: 10, weight: .bold)).foregroundStyle(.white)
                        .frame(width: 22, height: 22)
                        .background(.black.opacity(0.6), in: Circle())
                        .overlay(Circle().stroke(.white.opacity(0.28), lineWidth: 1))
                }
                .buttonStyle(.plain)
                .padding(6)
                .help("Close (Esc)")
                .accessibilityLabel("Close the larger picture")
            }
            .padding(8)
            .onExitCommand { close() }
            .accessibilityElement(children: .contain)
            .accessibilityLabel(label + ", larger")
    }
}

/// APP-889: how large a task's live preview shows when enlarged (16:9, as the short view's 300 × 170).
enum EnlargedPreview {
    static let size = CGSize(width: 640, height: 360)
}
