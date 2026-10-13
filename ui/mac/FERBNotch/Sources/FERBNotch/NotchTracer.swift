import SwiftUI
import NotchCore

/// Finite completion/attention glows share the native camera-edge geometry.
struct NotchTracerOverlay: View {
    @ObservedObject var model: NotchModel

    var body: some View {
        NotchTracer(active: model.showsTraceGlow,
                    completedAt: model.attentionTraceVisible ? model.attentionStartedAt : model.completionStartedAt,
                    color: model.attentionTraceVisible ? Palette.amber : Palette.green,
                    notchSize: CGSize(width: model.handleWidth, height: model.cutoutHeight))
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            .ignoresSafeArea()
            .allowsHitTesting(false)
            .accessibilityHidden(true)
    }
}

/// The open U follows the camera edge, rather than drawing a rounded rectangle
/// behind the hardware. Its path is allowed to sit anywhere in the glow canvas.
private struct NotchEdge: Shape {
    func path(in rect: CGRect) -> Path {
        let radius: CGFloat = min(10, rect.height)
        var path = Path()
        path.move(to: CGPoint(x: rect.minX, y: rect.minY))
        path.addLine(to: CGPoint(x: rect.minX, y: rect.maxY - radius))
        path.addQuadCurve(to: CGPoint(x: rect.minX + radius, y: rect.maxY),
                          control: CGPoint(x: rect.minX, y: rect.maxY))
        path.addLine(to: CGPoint(x: rect.maxX - radius, y: rect.maxY))
        path.addQuadCurve(to: CGPoint(x: rect.maxX, y: rect.maxY - radius),
                          control: CGPoint(x: rect.maxX, y: rect.maxY))
        path.addLine(to: CGPoint(x: rect.maxX, y: rect.minY))
        return path
    }
}

struct NotchTracer: View {
    let active: Bool
    let completedAt: Date?
    var color: Color = Palette.green
    let notchSize: CGSize
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.displayScale) private var displayScale

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 60, paused: !active)) { timeline in
            Canvas { context, canvas in
                guard active, let completedAt else { return }
                let edgeOffset = 0.5 / max(displayScale, 1)
                let rect = CGRect(x: (canvas.width - notchSize.width) / 2 - edgeOffset,
                                  y: 0, width: notchSize.width + edgeOffset * 2,
                                  height: notchSize.height + edgeOffset)
                let edge = NotchEdge().path(in: rect)
                let elapsed = timeline.date.timeIntervalSince(completedAt)
                context.opacity = CompletionGlow.opacity(at: elapsed, reduceMotion: reduceMotion)
                drawCompletionGlow(edge, in: &context)
            }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    private func drawCompletionGlow(_ edge: Path, in context: inout GraphicsContext) {
        context.drawLayer { glow in
            glow.addFilter(.blur(radius: 5))
            glow.stroke(edge, with: .color(color), style: StrokeStyle(lineWidth: 4, lineCap: .round))
        }
        context.drawLayer { glow in
            glow.addFilter(.blur(radius: 1.2))
            glow.stroke(edge, with: .color(color), style: StrokeStyle(lineWidth: 2, lineCap: .round))
        }
    }
}
