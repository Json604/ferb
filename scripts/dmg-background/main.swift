// The disk image's background (REL-409): a still frame of FERB's startup field (Gravity — Dense, APP-612) drawn by
// the app's own StartupStudyField around where FERB's icon sits, as the logo sits in the startup, with the window's
// words in the system font as FERB.app sets them. scripts/make_dmg_background.py compiles this file together with
// ui/mac/FERBNotch/Sources/FERBNotch/StartupStudies.swift and GravityDenseStudy.swift; never a hand-drawn imitation.
//
//   render SPEC.json OUT_PREFIX     writes OUT_PREFIX.png (1x) and OUT_PREFIX@2x.png from scripts/dmg_layout.py's spec
import AppKit
import SwiftUI

struct LayoutSpec: Decodable {
    let window: [Double]
    let icon_size: Double
    let icons: [String: [Double]]
    let app: String
    let link: String
    let heading: String
    let note: [String]
    let names: [String: String]
    let label_size: Double
    let label_center: Double
    let name_size: Double
}

/// The moment of the startup the still is taken from: inside the field's full presence (progress 0.12-0.72), before
/// the material settles (0.61), at a calm energy. The clip is about six seconds, so time follows progress.
enum StillFrame {
    static let progress = 0.2
    static let time = 0.2 * 6.0
    static let energy = 0.35
    static let opacity = 0.8
}

struct DiskImageBackground: View {
    let spec: LayoutSpec
    private let ink = Color(red: 0.018, green: 0.019, blue: 0.022)          // the startup stage's own black

    /// How wide Finder's label for an item is (the system font at the view's text size).
    private func labelWidth(_ item: String) -> Double {
        let name = spec.names[item] ?? item
        return (name as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: spec.label_size)]).width
    }

    var body: some View {
        let width = spec.window[0], height = spec.window[1], half = spec.icon_size / 2
        let app = spec.icons[spec.app]!, link = spec.icons[spec.link]!
        // The startup's own stage, scaled so its 150-point logo is FERB's icon: a 1512 x 982 screen around it.
        let stage = CGSize(width: 1512 * spec.icon_size / 150, height: 982 * spec.icon_size / 150)
        let arrowStart = app[0] + half + 28
        let arrowEnd = link[0] - half - 28
        ZStack(alignment: .topLeading) {
            ink
            StartupStudyField(study: .gravityDense, time: StillFrame.time, progress: StillFrame.progress,
                              energy: StillFrame.energy)
                .frame(width: stage.width, height: stage.height)
                .mask {
                    RadialGradient(colors: [.white, .white.opacity(0.9), .clear], center: .center,
                                   startRadius: 60 * spec.icon_size / 150, endRadius: stage.width * 0.65)
                }
                .opacity(StillFrame.opacity)
                .position(x: app[0], y: app[1])
            // Quiet the field under the words, so they read at full contrast
            LinearGradient(stops: [.init(color: ink.opacity(0.92), location: 0),
                                   .init(color: ink.opacity(0), location: 0.30),
                                   .init(color: ink.opacity(0), location: spec.note.isEmpty ? 1 : 0.70),
                                   .init(color: ink.opacity(spec.note.isEmpty ? 0 : 0.94), location: 1)],
                           startPoint: .top, endPoint: .bottom)
            // Finder's own (black) label sits just under each icon: keep the stage's black there, so it can't be seen
            ForEach([spec.app, spec.link], id: \.self) { item in
                let center = spec.icons[item]!
                Ellipse()
                    .fill(ink)
                    .frame(width: labelWidth(item) + 60, height: spec.label_size + 38)
                    .blur(radius: 10)
                    .position(x: center[0], y: center[1] + spec.label_center)
            }
            DragArrow()
                .frame(width: arrowEnd - arrowStart, height: 14)
                .position(x: (arrowStart + arrowEnd) / 2, y: app[1])
            // The names, in white, just below Finder's hidden labels
            ForEach([spec.app, spec.link], id: \.self) { item in
                let center = spec.icons[item]!
                Text(spec.names[item] ?? item)
                    .font(.system(size: spec.name_size, weight: .medium))
                    .foregroundStyle(.white.opacity(0.92))
                    .shadow(color: .black.opacity(0.6), radius: 3)
                    .fixedSize()
                    .position(x: center[0], y: center[1] + spec.label_center + spec.label_size / 2 + spec.name_size / 2 + 6)
            }
            Text(spec.heading)
                .font(.system(size: 22, weight: .semibold))
                .foregroundStyle(.white.opacity(0.94))
                .frame(width: width)
                .position(x: width / 2, y: 62)
            if spec.note.count == 2 {
                VStack(spacing: 5) {
                    Text(spec.note[0])
                        .font(.system(size: 13, weight: .medium))
                        .foregroundStyle(.white.opacity(0.9))
                    Text(spec.note[1])
                        .font(.system(size: 12))
                        .foregroundStyle(.white.opacity(0.62))
                        .lineSpacing(2)
                }
                .multilineTextAlignment(.center)
                .frame(width: 480)
                .position(x: width / 2, y: height - 58)
            }
        }
        .frame(width: width, height: height)
        .clipped()
        .environment(\.colorScheme, .dark)
    }
}

/// A thin line ending in an open chevron, pointing from FERB to Applications.
struct DragArrow: View {
    var body: some View {
        Canvas { context, size in
            var path = Path()
            let mid = size.height / 2
            path.move(to: CGPoint(x: 0, y: mid))
            path.addLine(to: CGPoint(x: size.width, y: mid))
            path.move(to: CGPoint(x: size.width - 6, y: mid - 6))
            path.addLine(to: CGPoint(x: size.width, y: mid))
            path.addLine(to: CGPoint(x: size.width - 6, y: mid + 6))
            context.stroke(path, with: .color(.white.opacity(0.42)),
                           style: StrokeStyle(lineWidth: 1.5, lineCap: .round, lineJoin: .round))
        }
    }
}

@MainActor
func writePNG(_ view: some View, scale: CGFloat, to url: URL) throws {
    let renderer = ImageRenderer(content: view)
    renderer.scale = scale
    renderer.isOpaque = true
    guard let image = renderer.cgImage else { throw NSError(domain: "render", code: 1) }
    let bitmap = NSBitmapImageRep(cgImage: image)
    bitmap.size = NSSize(width: Double(image.width) / scale, height: Double(image.height) / scale)  // 72 dpi per 1x
    guard let data = bitmap.representation(using: .png, properties: [:]) else { throw NSError(domain: "render", code: 2) }
    try data.write(to: url)
}

let arguments = CommandLine.arguments
guard arguments.count == 4, arguments[1] == "render" else {
    FileHandle.standardError.write("usage: render SPEC.json OUT_PREFIX\n".data(using: .utf8)!)
    exit(2)
}
do {
    let spec = try JSONDecoder().decode(LayoutSpec.self, from: Data(contentsOf: URL(fileURLWithPath: arguments[2])))
    try MainActor.assumeIsolated {
        let view = DiskImageBackground(spec: spec)
        try writePNG(view, scale: 1, to: URL(fileURLWithPath: arguments[3] + ".png"))
        try writePNG(view, scale: 2, to: URL(fileURLWithPath: arguments[3] + "@2x.png"))
    }
} catch {
    FileHandle.standardError.write("render: \(error)\n".data(using: .utf8)!)
    exit(1)
}
