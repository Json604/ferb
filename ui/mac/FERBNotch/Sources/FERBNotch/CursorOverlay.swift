import AppKit
import SwiftUI

struct PointerState {
    var target: CGPoint
    var kind: String
    var lastSeen: Date
    var hue: Double
    var display: CGDirectDisplayID
}

/// A transparent, click-through window over one display. It never takes the mouse or focus.
final class CursorPanel: NSPanel {
    init(screenFrame: CGRect) {
        super.init(contentRect: screenFrame, styleMask: [.borderless, .nonactivatingPanel],
                   backing: .buffered, defer: false)
        isOpaque = false
        backgroundColor = .clear
        hasShadow = false
        level = NSWindow.Level(rawValue: NSWindow.Level.statusBar.rawValue - 1)
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
    }
    /// Always click-through: a getter, so no assignment exists to flip it.
    override var ignoresMouseEvents: Bool { get { true } set {} }
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

/// One pointer per task, gliding to each element FERB acts on and fading after 1.5 s idle.
@MainActor
final class CursorController: ObservableObject {
    static let idleSeconds: TimeInterval = 1.5
    static let hueOffsets: [Double] = [0, 0.08, -0.08, 0.16, -0.16, 0.24, -0.24]

    @Published private(set) var pointers: [String: PointerState] = [:]
    var enabled: Bool {
        didSet { if !enabled { pointers.removeAll(); stopTimer(); closePanels() } }
    }
    var reduceMotion: Bool
    private let clock: () -> Date
    private let makePanels: Bool
    private var hues: [String: Double] = [:]
    private var panels: [CGDirectDisplayID: CursorPanel] = [:]
    private var timer: Timer?

    init(clock: @escaping () -> Date = Date.init, makePanels: Bool = true) {
        self.clock = clock
        self.makePanels = makePanels
        self.enabled = UserDefaults.standard.object(forKey: "cursor.enabled") as? Bool ?? true
        self.reduceMotion = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
    }

    func point(taskID: String, kind: String, rect: CGRect, display: CGDirectDisplayID) {
        guard enabled else { return }
        let target: CGPoint
        switch kind {
        case "type": target = CGPoint(x: rect.minX + 12, y: rect.midY)
        default: target = CGPoint(x: rect.midX, y: rect.midY)
        }
        if hues[taskID] == nil {
            hues[taskID] = Self.hueOffsets[hues.count % Self.hueOffsets.count]
        }
        reduceMotion = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        pointers[taskID] = PointerState(target: target, kind: kind, lastSeen: clock(),
                                        hue: hues[taskID] ?? 0, display: display)
        if makePanels { showPanel(for: display); startTimer() }
    }

    func end(taskID: String) {
        pointers[taskID] = nil
        hues[taskID] = nil
        if pointers.isEmpty { stopTimer() }
    }

    func sweep() {
        let now = clock()
        for (id, state) in pointers where now.timeIntervalSince(state.lastSeen) > Self.idleSeconds {
            pointers[id] = nil
        }
        if pointers.isEmpty { stopTimer() }
    }

    private func startTimer() {
        guard timer == nil else { return }
        timer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.sweep() }
        }
    }

    private func stopTimer() { timer?.invalidate(); timer = nil }

    private func closePanels() {
        panels.values.forEach { $0.orderOut(nil) }
        panels.removeAll()
    }

    /// Global points are top-left origin (display bounds); the panel takes the screen's AppKit frame and its
    /// SwiftUI content is top-left too, so a pointer lands at its target minus the display's origin.
    private func showPanel(for display: CGDirectDisplayID) {
        if let panel = panels[display] { panel.orderFrontRegardless(); return }
        guard let screen = NSScreen.screens.first(where: {
            ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value == display
        }) else { return }
        let panel = CursorPanel(screenFrame: screen.frame)
        let origin = CGDisplayBounds(display).origin
        panel.contentView = NSHostingView(rootView: CursorLayerView(controller: self, display: display, origin: origin))
        panel.setFrame(screen.frame, display: false)
        panel.orderFrontRegardless()
        panels[display] = panel
    }
}

struct CursorLayerView: View {
    @ObservedObject var controller: CursorController
    let display: CGDirectDisplayID
    let origin: CGPoint

    var body: some View {
        ZStack(alignment: .topLeading) {
            ForEach(controller.pointers.sorted(by: { $0.key < $1.key }).filter { $0.value.display == display }, id: \.key) { _, state in
                CursorPointerView(hue: state.hue)
                    .position(x: state.target.x - origin.x + 8.5, y: state.target.y - origin.y + 12)
                    .transition(.opacity)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .animation(controller.reduceMotion ? .easeOut(duration: 0.15) : .spring(response: 0.35, dampingFraction: 1.0),
                   value: controller.pointers.mapValues { $0.target.x + $0.target.y * 10_000 })
        .animation(.easeOut(duration: controller.reduceMotion ? 0.15 : 0.3), value: controller.pointers.count)
    }
}
