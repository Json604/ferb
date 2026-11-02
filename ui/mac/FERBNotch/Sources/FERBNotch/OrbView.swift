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

/// The frosted bubble beside the orb: one line and an optional reason line.
struct InstructionBubble: View {
    let line: IntroLine

    var body: some View {
        HStack(spacing: 8) {
            if let path = line.iconPath {
                Image(nsImage: NSWorkspace.shared.icon(forFile: path))
                    .resizable().frame(width: 26, height: 26).accessibilityHidden(true)
            }
            VStack(alignment: .leading, spacing: 3) {
                Text(line.text).font(.system(size: 13, weight: .medium))
                if let detail = line.detail, !detail.isEmpty {
                    Text(detail).font(.system(size: 11)).foregroundStyle(.white.opacity(0.7))
                }
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
/// (the file URL a privacy list accepts). A faint fill makes the whole card grabbable, not only the icon's opaque
/// pixels (the window server passes clicks on clear pixels through). A click alone does nothing.
final class DragCardView: NSView, NSDraggingSource {
    /// The owner started dragging the logo (the drag demonstration stops).
    var onDragBegan: (() -> Void)?
    var onDropAccepted: (() -> Void)?
    var url: URL? {
        didSet {
            icon.image = NSWorkspace.shared.icon(forFile: url?.path ?? Bundle.main.bundlePath)
            window?.invalidateCursorRects(for: self)
        }
    }
    private let icon = NSImageView()

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 10
        layer?.backgroundColor = NSColor.clear.cgColor
        icon.image = NSWorkspace.shared.icon(forFile: Bundle.main.bundlePath)
        icon.frame = bounds
        icon.autoresizingMask = [.width, .height]
        autoresizingMask = [.width, .height]
        icon.imageScaling = .scaleProportionallyUpOrDown
        icon.unregisterDraggedTypes()
        addSubview(icon)
        setAccessibilityElement(true)
        setAccessibilityRole(.button)
        setAccessibilityLabel("FERB, drag into the list")
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
        onDragBegan?()
        let item = NSDraggingItem(pasteboardWriter: url as NSURL)
        item.setDraggingFrame(bounds, contents: icon.image)
        beginDraggingSession(with: [item], event: event, source: self)
    }

    func draggingSession(_ session: NSDraggingSession, endedAt screenPoint: NSPoint, operation: NSDragOperation) {
        if !operation.isEmpty { onDropAccepted?() }
    }

    func draggingSession(_ session: NSDraggingSession,
                         sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        [.copy, .link, .generic]
    }
}
