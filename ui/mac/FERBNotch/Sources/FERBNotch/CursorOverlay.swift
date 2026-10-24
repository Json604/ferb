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
        hidesOnDeactivate = false
        level = NSWindow.Level(rawValue: NSWindow.Level.statusBar.rawValue - 1)
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
    }
    /// Always click-through: a getter, so no assignment exists to flip it.
    override var ignoresMouseEvents: Bool { get { true } set {} }
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

/// One pointer per task (the daemon names each point by its step's root task), gliding to each element FERB acts on
/// and fading after 1.5 s idle; the task's result ends it.
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
    private var screenObserver: NSObjectProtocol?

    /// A global top-left point expressed in a display's own top-left space.
    nonisolated static func localPoint(target: CGPoint, displayOrigin: CGPoint) -> CGPoint {
        CGPoint(x: target.x - displayOrigin.x, y: target.y - displayOrigin.y)
    }

    init(clock: @escaping () -> Date = Date.init, makePanels: Bool = true) {
        self.clock = clock
        self.makePanels = makePanels
        self.enabled = UserDefaults.standard.object(forKey: "cursor.enabled") as? Bool ?? true
        self.reduceMotion = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        if makePanels {
            screenObserver = NotificationCenter.default.addObserver(
                forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.closePanels() }   // the next point recreates them
            }
        }
    }

    func point(taskID: String, kind: String, rect: CGRect, display: CGDirectDisplayID) {
        guard enabled else { return }
        let target: CGPoint
        switch kind {
        // the field's leading edge, never past its middle (the daemon clips a field to the part its page shows)
        case "type": target = CGPoint(x: min(rect.minX + 12, rect.midX), y: rect.midY)
        default: target = CGPoint(x: rect.midX, y: rect.midY)
        }
        reduceMotion = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        pointers[taskID] = PointerState(target: target, kind: kind, lastSeen: clock(),
                                        hue: hue(for: taskID), display: display)
        if makePanels { showPanel(for: display); startTimer() }
    }

    /// A task keeps its shade until it ends, across the idle gaps between its steps; a new task takes a shade no
    /// other task holds, while one is free.
    private func hue(for taskID: String) -> Double {
        if let hue = hues[taskID] { return hue }
        let taken = Set(hues.values)
        let hue = Self.hueOffsets.first { !taken.contains($0) } ?? Self.hueOffsets[hues.count % Self.hueOffsets.count]
        hues[taskID] = hue
        return hue
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
    private func local(_ s: PointerState) -> CGPoint { CursorController.localPoint(target: s.target, displayOrigin: origin) }

    var body: some View {
        ZStack(alignment: .topLeading) {
            ForEach(controller.pointers.sorted(by: { $0.key < $1.key }).filter { $0.value.display == display }, id: \.key) { _, state in
                CursorPointerView(hue: state.hue)
                    .position(x: local(state).x + 8.5, y: local(state).y + 12)
                    .transition(.opacity)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .ignoresSafeArea()
        .animation(controller.reduceMotion ? nil : .spring(response: 0.35, dampingFraction: 1.0),
                   value: controller.pointers.mapValues { $0.target.x + $0.target.y * 10_000 })
        .animation(.easeOut(duration: controller.reduceMotion ? 0.15 : 0.3), value: controller.pointers.count)
    }
}
