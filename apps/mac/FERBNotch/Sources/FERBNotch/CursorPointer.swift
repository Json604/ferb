import SwiftUI

/// The macOS-cursor-proportioned arrow, drawn with its tip at the view's top-left corner.
struct CursorArrow: Shape {
    func path(in rect: CGRect) -> Path {
        let w = rect.width, h = rect.height
        var p = Path()
        p.move(to: CGPoint(x: 0, y: 0))
        p.addLine(to: CGPoint(x: 0, y: h * 0.86))
        p.addLine(to: CGPoint(x: w * 0.27, y: h * 0.66))
        p.addLine(to: CGPoint(x: w * 0.47, y: h))
        p.addLine(to: CGPoint(x: w * 0.65, y: h * 0.92))
        p.addLine(to: CGPoint(x: w * 0.46, y: h * 0.59))
        p.addLine(to: CGPoint(x: w * 0.82, y: h * 0.59))
        p.closeSubpath()
        return p
    }
}

struct CursorPointerView: View {
    let hue: Double

    var body: some View {
        CursorArrow()
            .fill(Color.accentColor)
            .overlay(CursorArrow().stroke(Color.white, lineWidth: 1.5))
            .hueRotation(.degrees(hue * 360))
            .frame(width: 17, height: 24)
            .shadow(color: .black.opacity(0.35), radius: 3)
    }
}
