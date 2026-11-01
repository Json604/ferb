import AppKit
import SwiftUI
import NotchCore

/// What the orb's canvas shows; OrbPanel sets it, OrbController drives OrbPanel.
@MainActor
final class OrbViewState: ObservableObject {
    @Published var scale: CGFloat = 1
    @Published var opacity: CGFloat = 0
    @Published var instruction: IntroLine?
    @Published var waiting = false
    @Published var holding = false
    @Published var bubbleOnLeft = false
}

struct OrbCanvas: View {
    @ObservedObject var state: OrbViewState

    var body: some View {
        ZStack {
            OrbGlass(waiting: state.waiting)
                .frame(width: OrbController.size.width, height: OrbController.size.height)
                .scaleEffect(state.scale)
            if let line = state.instruction {
                InstructionBubble(line: line)
                    .frame(width: OrbPanel.bubbleWidth, alignment: state.bubbleOnLeft ? .trailing : .leading)
                    .offset(x: (state.bubbleOnLeft ? -1 : 1) * (OrbController.size.width / 2 + 12 + OrbPanel.bubbleWidth / 2))
                    .transition(.opacity)
            }
        }
        .opacity(Double(state.opacity))
        .frame(width: OrbPanel.canvas.width, height: OrbPanel.canvas.height)
        .animation(.easeOut(duration: 0.2), value: state.instruction)
    }
}

/// A small glass orb with a soft iridescent band of light inside that ripples while FERB waits.
struct OrbGlass: View {
    let waiting: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        TimelineView(.animation(paused: !waiting || reduceMotion)) { context in
            let t = context.date.timeIntervalSinceReferenceDate
            let ripple = waiting && !reduceMotion ? sin(t * 2 * .pi / 1.8) : 0
            ZStack {
                Circle().fill(RadialGradient(colors: [.white.opacity(0.3), .white.opacity(0.06)],
                                             center: UnitPoint(x: 0.35, y: 0.3), startRadius: 2, endRadius: 34))
                Circle()
                    .fill(AngularGradient(colors: [.cyan, .purple, .pink, .orange, .cyan], center: .center,
                                          angle: .degrees(waiting && !reduceMotion ? t * 40 : 30)))
                    .mask { Ellipse().frame(height: 14 + 4 * ripple).blur(radius: 4) }
                    .rotationEffect(.degrees(-20))
                    .opacity(0.55 + 0.15 * ripple)
                    .blur(radius: 3)
                Circle().strokeBorder(LinearGradient(colors: [.white.opacity(0.7), .white.opacity(0.15)],
                                                     startPoint: .topLeading, endPoint: .bottomTrailing), lineWidth: 1)
                Ellipse().fill(.white.opacity(0.55)).frame(width: 18, height: 8).blur(radius: 2).offset(x: -9, y: -15)
            }
            .clipShape(Circle())
            .scaleEffect(1 + 0.02 * ripple)
            .shadow(color: .black.opacity(0.25), radius: 6, y: 2)
        }
        .accessibilityHidden(true)
    }
}

/// The frosted bubble beside the orb: one line and an optional reason line.
struct InstructionBubble: View {
    let line: IntroLine

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(line.text).font(.system(size: 13, weight: .medium))
            if let detail = line.detail, !detail.isEmpty {
                Text(detail).font(.system(size: 11)).foregroundStyle(.white.opacity(0.7))
            }
        }
        .foregroundStyle(.white)
        .fixedSize(horizontal: false, vertical: true)
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .background(Color(white: 0.1, opacity: 0.78), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(.white.opacity(0.12)))
        .accessibilityElement(children: .combine)
    }
}

/// The FERB icon the orb holds, to drag into a System Settings list: a drag source for the agent app's bundle URL
/// (the file URL a privacy list accepts). A click alone does nothing.
final class DragCardView: NSView, NSDraggingSource {
    var url: URL? {
        didSet {
            icon.image = url.map { NSWorkspace.shared.icon(forFile: $0.path) }
            setAccessibilityLabel(url.map { "\($0.deletingPathExtension().lastPathComponent). Drag into the list." })
            window?.invalidateCursorRects(for: self)
        }
    }
    private let icon = NSImageView()

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 10
        icon.frame = bounds.insetBy(dx: 4, dy: 4)
        icon.imageScaling = .scaleProportionallyUpOrDown
        icon.unregisterDraggedTypes()
        addSubview(icon)
        setAccessibilityElement(true)
        setAccessibilityRole(.button)
    }

    required init?(coder: NSCoder) { nil }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { frame.contains(point) ? self : nil }
    override func resetCursorRects() {
        if url != nil { addCursorRect(bounds, cursor: .openHand) }
    }
    override func mouseDown(with event: NSEvent) {}

    override func mouseDragged(with event: NSEvent) {
        guard let url else { return }
        let item = NSDraggingItem(pasteboardWriter: url as NSURL)
        item.setDraggingFrame(bounds, contents: icon.image)
        beginDraggingSession(with: [item], event: event, source: self)
    }

    func draggingSession(_ session: NSDraggingSession,
                         sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        context == .outsideApplication ? [.copy, .link, .generic] : []
    }
}
