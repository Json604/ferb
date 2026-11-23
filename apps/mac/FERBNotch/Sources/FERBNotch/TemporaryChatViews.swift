import SwiftUI

/// A chat bubble with its tail at the lower left, drawn as one outline: the temporary chat's icon strokes it dashed
/// (the same shape on the Mac and both phones).
struct DashedBubbleShape: Shape {
    func path(in rect: CGRect) -> Path {
        let body = CGRect(x: rect.minX, y: rect.minY, width: rect.width, height: rect.height * 0.8)
        let radius = min(body.width, body.height) * 0.38
        var path = Path()
        path.move(to: CGPoint(x: body.minX + radius, y: body.minY))
        path.addLine(to: CGPoint(x: body.maxX - radius, y: body.minY))
        path.addArc(center: CGPoint(x: body.maxX - radius, y: body.minY + radius), radius: radius,
                    startAngle: .degrees(-90), endAngle: .degrees(0), clockwise: false)
        path.addLine(to: CGPoint(x: body.maxX, y: body.maxY - radius))
        path.addArc(center: CGPoint(x: body.maxX - radius, y: body.maxY - radius), radius: radius,
                    startAngle: .degrees(0), endAngle: .degrees(90), clockwise: false)
        // the tail, down to the lower left
        path.addLine(to: CGPoint(x: body.minX + body.width * 0.42, y: body.maxY))
        path.addLine(to: CGPoint(x: body.minX + body.width * 0.14, y: rect.maxY))
        path.addLine(to: CGPoint(x: body.minX + body.width * 0.2, y: body.maxY))
        path.addLine(to: CGPoint(x: body.minX + radius, y: body.maxY))
        path.addArc(center: CGPoint(x: body.minX + radius, y: body.maxY - radius), radius: radius,
                    startAngle: .degrees(90), endAngle: .degrees(180), clockwise: false)
        path.addLine(to: CGPoint(x: body.minX, y: body.minY + radius))
        path.addArc(center: CGPoint(x: body.minX + radius, y: body.minY + radius), radius: radius,
                    startAngle: .degrees(180), endAngle: .degrees(270), clockwise: false)
        path.closeSubpath()
        return path
    }
}

/// The dashed bubble drawn at `size`: an outline while off; filled with the accent, its dashes still showing, while on.
struct DashedBubbleGlyph: View {
    var on: Bool
    var size: CGFloat = 16

    var body: some View {
        ZStack {
            if on { DashedBubbleShape().fill(Palette.chatBlue.opacity(0.85)) }
            DashedBubbleShape()
                .stroke(on ? Color.white : Color.white.opacity(0.65),
                        style: StrokeStyle(lineWidth: 1.4, lineCap: .round, dash: [2.6, 2.2]))
        }
        .frame(width: size, height: size * 0.9)
    }
}

/// The temporary chat's one switch, in the chat's header at the top right (the notch's and the window's): a tap turns
/// it on, the same tap turns it off (owner 2026-11-23: "it shodln be the same small icon that turned it on taht turns it
/// off").
struct TemporaryChatButton: View {
    @ObservedObject var model: NotchModel

    var body: some View {
        let on = model.inTemporaryChat
        Button(action: model.toggleTemporaryChat) {
            DashedBubbleGlyph(on: on)
                .frame(width: 26, height: 26)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(model.temporaryToggling)
        .help(on ? NotchModel.endTemporaryChatLabel : NotchModel.temporaryChatLabel)
        .accessibilityLabel(on ? NotchModel.endTemporaryChatLabel : NotchModel.temporaryChatLabel)
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}

/// The line at the top of the chat while the temporary chat is on, in the days' row's place and height.
struct TemporaryChatBanner: View {
    var body: some View {
        HStack(spacing: 6) {
            DashedBubbleGlyph(on: true, size: 11)
            Text(NotchModel.temporaryChatBanner)
                .font(.system(size: 11, weight: .medium))
                .lineLimit(1).minimumScaleFactor(0.75)
            Spacer(minLength: 0)
        }
        .foregroundStyle(Palette.ink)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(NotchModel.temporaryChatBanner)
    }
}
