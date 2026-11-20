import AppKit
import LocalAuthentication
import NotchCore
import Combine

// Insomnia mode's cover (APP-980..APP-985): while the owner is away and the mode is on with "Cover the screen", every
// display shows FERB's cover; the Mac stays unlocked underneath so FERB keeps working, and only the owner's Touch ID or
// password removes it. The recipe is the one measured on the owner's Mac (~/Desktop/ferb-agent-remote/spikes/x1-shield):
// a regular-policy app showing an NSPanel per display that joins every app's and every Space (a plain window stays
// silently off screen over a full-screen app, and an accessory app lets Cmd-Tab through), at the shielding level, with
// the kiosk options while it is the active app. Touch ID's prompt opens at layer 1000, so the cover steps down to 999
// only while it asks. NotchCore's InsomniaController decides every state; this file only does what it says.

/// One display's cover.
final class ShieldPanel: NSPanel {
    /// Above everything but the lock screen (`CGShieldingWindowLevel()`).
    static let coverLevel = NSWindow.Level(rawValue: Int(CGShieldingWindowLevel()))
    /// Just under the system's Touch ID/password prompt (layer 1000), while FERB.app asks it (x1 experiment 7).
    static let askingLevel = NSWindow.Level(rawValue: 999)
    static let behavior: NSWindow.CollectionBehavior = [.canJoinAllApplications, .canJoinAllSpaces, .fullScreenAuxiliary,
                                                        .stationary, .ignoresCycle]

    override var canBecomeKey: Bool { true }        // typing lands here (and is dropped), never in an app beneath
    override var canBecomeMain: Bool { false }
    /// Passes clicks through to the app beneath during a step, or catches them again (x1 experiment 3): the cover's one
    /// write of `ignoresMouseEvents` (an opaque window: APP-560's transparent-area concern doesn't arise).
    func passClicks(_ on: Bool) {
        ignoresMouseEvents = on
    }

    /// A cover for `frame`, configured and not ordered in.
    static func make(frame: CGRect) -> ShieldPanel {
        let panel = ShieldPanel(contentRect: frame, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered,
                                defer: true)
        panel.level = coverLevel
        panel.collectionBehavior = behavior
        panel.isOpaque = true
        panel.backgroundColor = .black
        panel.hasShadow = false
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.isMovable = false
        panel.becomesKeyOnlyIfNeeded = false
        panel.worksWhenModal = true                 // a modal run (an update's alert) never makes Unlock dead
        panel.animationBehavior = .none
        panel.title = "FERB cover"
        return panel
    }
}

// MARK: - The look (APP-1100, for APP-985: one swap point)

/// What every display's look shows besides time: dimmed (APP-982), asking, the daemon's line, and Unlock.
@MainActor
final class ShieldLookState: ObservableObject {
    @Published var dimmed = false
    /// Touch ID or the password is being asked.
    @Published var asking = false
    /// The daemon's status line (what FERB is doing).
    @Published var line = ""
    var unlock: () -> Void = {}
}

/// The cover's look on one display. The frame clock (InsomniaCoverWindows) says when to draw a frame; the look draws
/// it, fades in, dims and dismisses. `ShieldLookInUse` is the one place the look is chosen.
@MainActor
protocol ShieldLook: AnyObject {
    /// `main`: the display that shows the mark and the lines (the others show the field).
    init(size: CGSize, scale: CGFloat, main: Bool, state: ShieldLookState, firstName: String)
    var view: NSView { get }
    /// The look as it is at `now` (one frame).
    func draw(_ timeline: ShieldLookTimeline, at now: CFTimeInterval)
    /// In from black (the cover itself is opaque black from its first frame).
    func enter(reduced: Bool)
    /// Fades to near-black or back (APP-982); the frame clock stops drawing while dimmed.
    func setDimmed(_ dimmed: Bool, reduced: Bool)
    /// Touch ID accepted (ShieldDismiss): the mark, the lines, then the field, to black.
    func dismiss(reduced: Bool)
}

/// The look in use.
typealias ShieldLookInUse = FrostedGrainLook

/// The owner's choice (APP-1100): the startup's field fully frosted (ShieldField.swift), the f written, then the dot
/// breathing (ShieldMarkMotion), the prototype's lines.
@MainActor
final class FrostedGrainLook: ShieldLook {
    let view: NSView
    private let content = NSView()
    private let dim = NSView()
    private let fieldView: ShieldFieldView?
    private let mark: ShieldMarkLayers?
    private var lines: [NSTextField] = []
    private var hint: NSTextField?
    private var hintSymbol: NSImageView?
    private var task: NSTextField?
    private var watch: [Any] = []

    init(size: CGSize, scale: CGFloat, main: Bool, state: ShieldLookState, firstName: String) {
        let bounds = CGRect(origin: .zero, size: size)
        view = NSView(frame: bounds)
        view.wantsLayer = true
        view.layer?.backgroundColor = NSColor.black.cgColor
        view.layer?.isOpaque = true
        content.frame = bounds
        content.autoresizingMask = [.width, .height]
        content.wantsLayer = true
        view.addSubview(content)
        fieldView = ShieldFieldPipeline.shared.map { ShieldFieldView(frame: bounds, pipeline: $0, scale: scale) }
        if let fieldView { fieldView.autoresizingMask = [.width, .height]; content.addSubview(fieldView) }
        if main {
            let overlay = NSView(frame: bounds)
            overlay.autoresizingMask = [.width, .height]
            overlay.wantsLayer = true
            content.addSubview(overlay)
            let mark = ShieldMarkLayers(unit: Self.unit, scale: scale)
            mark.container.position = CGPoint(x: size.width / 2, y: size.height / 2)   // the field's centre, its vortex
            overlay.layer?.addSublayer(mark.container)
            self.mark = mark
            let markBottom = size.height / 2 - (93 - 64) * Self.unit
            let first = Self.label(ShieldWords.lockedTo(firstName), size: 15, weight: .medium, alpha: 0.9)
            first.frame = NSRect(x: 0, y: markBottom - 48, width: size.width, height: 20)
            let task = Self.label(state.line, size: 13, alpha: 0.55)
            task.frame = NSRect(x: 0, y: markBottom - 70, width: size.width, height: 18)
            let hint = Self.label(ShieldWords.unlock, size: 13, alpha: 0.6)
            hint.alignment = .left
            hint.frame = NSRect(x: size.width / 2 - 70, y: 70, width: 220, height: 18)
            let symbol = NSImageView(image: NSImage(systemSymbolName: "touchid", accessibilityDescription: nil) ?? NSImage())
            symbol.symbolConfiguration = .init(pointSize: 15, weight: .regular)
            symbol.contentTintColor = NSColor(white: 1, alpha: 0.6)
            symbol.frame = NSRect(x: size.width / 2 - 94, y: 70, width: 18, height: 18)
            for item in [first, task, hint, symbol] as [NSView] { overlay.addSubview(item) }
            lines = [first, task, hint]
            self.hint = hint
            self.task = task
            hintSymbol = symbol
            watch.append(state.$line.sink { [weak task] line in task?.stringValue = line })
            watch.append(state.$asking.sink { [weak hint] asking in
                hint?.stringValue = asking ? ShieldWords.waiting : ShieldWords.unlock
            })
        } else {
            mark = nil
        }
        dim.frame = bounds
        dim.autoresizingMask = [.width, .height]
        dim.wantsLayer = true
        dim.layer?.backgroundColor = NSColor.black.cgColor
        dim.alphaValue = 0
        view.addSubview(dim)
        mark?.apply(.still)
    }

    /// The mark's size: logo units to points (the prototype's).
    static let unit = 1.85
    /// Near-black when stopped (APP-982).
    static let dimmedAlpha: CGFloat = 0.92

    /// The frosted field in use on this display (tests read its blur).
    var field: ShieldFieldRenderer? { fieldView?.renderer }

    func draw(_ timeline: ShieldLookTimeline, at now: CFTimeInterval) {
        let frame = timeline.mark(at: now)
        fieldView?.renderer.draw(ShieldFieldFrame(time: timeline.fieldTime(at: now), energy: frame.energy,
                                                  crest: frame.crest), size: view.bounds.size)
        mark?.apply(frame)
    }

    func enter(reduced: Bool) {
        content.alphaValue = 0
        NSAnimationContext.runAnimationGroup { context in
            context.duration = reduced ? 0.3 : 0.5
            context.timingFunction = Self.ease
            content.animator().alphaValue = 1
        }
    }

    func setDimmed(_ dimmed: Bool, reduced: Bool) {
        NSAnimationContext.runAnimationGroup { context in
            context.duration = reduced ? 0.3 : 2
            dim.animator().alphaValue = dimmed ? Self.dimmedAlpha : 0
        }
    }

    func dismiss(reduced: Bool) {
        dim.alphaValue = 0
        if reduced {
            NSAnimationContext.runAnimationGroup { context in
                context.duration = ShieldDismiss.reducedLength
                content.animator().alphaValue = 0
            }
            return
        }
        mark?.dismiss()
        NSAnimationContext.runAnimationGroup { context in
            context.duration = ShieldDismiss.text.length
            for item in lines { item.animator().alphaValue = 0 }
            hintSymbol?.animator().alphaValue = 0
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + ShieldDismiss.field.start) { [content] in
            NSAnimationContext.runAnimationGroup { context in
                context.duration = ShieldDismiss.field.length
                context.timingFunction = Self.ease
                content.animator().alphaValue = 0
            }
        }
    }

    static let ease = CAMediaTimingFunction(controlPoints: Float(ShieldDismiss.ease.0), Float(ShieldDismiss.ease.1),
                                            Float(ShieldDismiss.ease.2), Float(ShieldDismiss.ease.3))

    private static func label(_ text: String, size: CGFloat, weight: NSFont.Weight = .regular,
                              alpha: CGFloat) -> NSTextField {
        let label = NSTextField(labelWithString: text)
        label.font = .systemFont(ofSize: size, weight: weight)
        label.textColor = NSColor(white: 1, alpha: alpha)
        label.alignment = .center
        label.lineBreakMode = .byTruncatingTail
        return label
    }
}

/// Hosts one display's CAMetalLayer.
final class ShieldFieldView: NSView {
    let renderer: ShieldFieldRenderer

    init(frame: CGRect, pipeline: ShieldFieldPipeline, scale: CGFloat) {
        renderer = ShieldFieldRenderer(pipeline: pipeline, pixelsPerPoint: scale)
        super.init(frame: frame)
        wantsLayer = true
        layerContentsRedrawPolicy = .never
    }

    required init?(coder: NSCoder) { nil }

    override func makeBackingLayer() -> CALayer { renderer.layer }
}

/// FERB's mark (site/logo.svg's f, MenuBarIcon.Logo, and its amber dot) as layers, set once per frame.
@MainActor
final class ShieldMarkLayers {
    let container = CALayer()
    let f = CAShapeLayer()
    let pen = CAShapeLayer()
    let dot = CAShapeLayer()
    let glow = CAGradientLayer()
    let unit: CGFloat
    /// The logo's amber (site/logo.svg).
    static let amber = NSColor(srgbRed: 0xF5 / 255.0, green: 0xA5 / 255.0, blue: 0x24 / 255.0, alpha: 1)

    init(unit: CGFloat, scale: CGFloat) {
        self.unit = unit
        let side = 128 * unit
        container.bounds = CGRect(x: 0, y: 0, width: side, height: side)
        container.isGeometryFlipped = true
        f.path = Self.letter(unit)
        f.fillColor = NSColor(white: 0.97, alpha: 1).cgColor
        f.frame = container.bounds
        // The pen's route, stroked wide as a mask: the hook, down the stem, then the crossbar (hand order).
        let route = CGMutablePath()
        func pt(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: x * unit, y: y * unit) }
        route.move(to: pt(69.5, 41.3))
        route.addLine(to: pt(63.5, 41.15))
        route.addQuadCurve(to: pt(55.22, 49.8), control: pt(55.22, 41.0))
        route.addLine(to: pt(55.22, 92))
        route.move(to: pt(41.5, 55.26))
        route.addLine(to: pt(70, 55.26))
        pen.path = route
        pen.strokeColor = NSColor.black.cgColor
        pen.fillColor = nil
        pen.lineWidth = 15 * unit
        pen.lineCap = .round
        pen.lineJoin = .round
        pen.frame = container.bounds
        f.mask = pen
        let r = MenuBarIcon.Logo.dotRadius * unit
        glow.type = .radial
        glow.colors = [Self.amber.withAlphaComponent(0.55).cgColor, Self.amber.withAlphaComponent(0.18).cgColor,
                       Self.amber.withAlphaComponent(0).cgColor]
        glow.locations = [0, 0.35, 1]
        glow.startPoint = CGPoint(x: 0.5, y: 0.5)
        glow.endPoint = CGPoint(x: 1, y: 1)
        glow.bounds = CGRect(x: 0, y: 0, width: r * 7, height: r * 7)
        dot.bounds = CGRect(x: 0, y: 0, width: 2 * r, height: 2 * r)
        dot.path = CGPath(ellipseIn: dot.bounds, transform: nil)
        dot.fillColor = Self.amber.cgColor
        for layer in [container, f, pen, dot, glow] { layer.contentsScale = scale }
        container.addSublayer(glow)
        container.addSublayer(f)
        container.addSublayer(dot)
    }

    private static func letter(_ unit: CGFloat) -> CGPath {
        let path = CGMutablePath()
        func pt(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: x * unit, y: y * unit) }
        for op in MenuBarIcon.Logo.f {
            switch op {
            case .move(let x, let y): path.move(to: pt(x, y))
            case .line(let x, let y): path.addLine(to: pt(x, y))
            case .quad(let cx, let cy, let x, let y): path.addQuadCurve(to: pt(x, y), control: pt(cx, cy))
            }
        }
        path.closeSubpath()
        return path
    }

    func apply(_ m: ShieldMarkFrame) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        let home = MenuBarIcon.Logo.dot
        let at = CGPoint(x: home.x * unit, y: (home.y - CGFloat(m.dotLift)) * unit)
        dot.position = at
        dot.opacity = Float(m.dotAlpha)
        dot.transform = CATransform3DMakeScale(CGFloat(m.dotScale), CGFloat(m.dotScale), 1)
        glow.position = at
        glow.opacity = Float(m.glow * m.dotAlpha)
        glow.transform = CATransform3DMakeScale(CGFloat(m.dotScale * (0.9 + 0.3 * m.glow)),
                                                CGFloat(m.dotScale * (0.9 + 0.3 * m.glow)), 1)
        pen.strokeEnd = CGFloat(m.reveal)
        f.opacity = Float(m.fAlpha)
        CATransaction.commit()
    }

    /// The dot glows once, then the mark shrinks 6 % and fades (ShieldDismiss).
    func dismiss() {
        let now = CACurrentMediaTime()
        func animate(_ layer: CALayer, _ key: String, from: Any, to: Any, start: Double, length: Double) {
            let a = CABasicAnimation(keyPath: key)
            a.fromValue = from
            a.toValue = to
            a.beginTime = layer.convertTime(now + start, from: nil)
            a.duration = length
            a.timingFunction = FrostedGrainLook.ease
            a.fillMode = .both
            a.isRemovedOnCompletion = false
            layer.add(a, forKey: "dismiss-" + key)
        }
        animate(glow, "opacity", from: glow.opacity, to: 1, start: ShieldDismiss.glow.start, length: ShieldDismiss.glow.length)
        animate(container, "opacity", from: 1, to: 0, start: ShieldDismiss.mark.start, length: ShieldDismiss.mark.length)
        animate(container, "transform.scale", from: 1, to: ShieldDismiss.mark.scale, start: ShieldDismiss.mark.start,
                length: ShieldDismiss.mark.length)
    }
}

// MARK: - The windows and watchers

/// The cover's windows, the person-input tap and the activation watch, for InsomniaController. Made only in the
/// packaged FERB.app (AppDelegate); nothing here runs until the controller raises the cover.
@MainActor
final class InsomniaCoverWindows: InsomniaCoverSurface {
    static let kiosk: NSApplication.PresentationOptions = [.hideDock, .hideMenuBar, .disableProcessSwitching,
                                                           .disableForceQuit, .disableSessionTermination,
                                                           .disableHideApplication, .disableAppleMenu]

    private weak var controller: InsomniaController?
    private let state = ShieldLookState()
    /// Whether anyone can see the cover: the displays awake, the screen unlocked (the previews' gate, APP-407).
    private let access: PreviewAccess
    private var accessWatch: AnyCancellable?
    private(set) var looks: [ShieldLookInUse] = []
    private(set) var timeline: ShieldLookTimeline?
    private var frames: Timer?
    private var framesPerSecond = 0
    /// Counts raises, so a dismiss that ends after a new raise leaves the new cover alone.
    private var generation = 0
    private let firstName = ShieldWords.firstName(fullName: NSFullUserName(), account: NSUserName())
    private lazy var camera = InsomniaCamera(rpc: rpc) { [weak self] reported in self?.controller?.cameraChanged(reported) }
    private let rpc: DaemonRPCTransport
    private(set) var panels: [ShieldPanel] = []
    private var clickThrough = false
    private var tap: CFMachPort?
    private var tapSource: CFRunLoopSource?
    private var observers: [(NotificationCenter, NSObjectProtocol)] = []
    private var monitor: Any?
    private var timer: Timer?

    init(controller: InsomniaController, rpc: DaemonRPCTransport, access: PreviewAccess) {
        self.controller = controller
        self.rpc = rpc
        self.access = access
        state.unlock = { [weak controller] in controller?.unlock() }
    }

    // MARK: InsomniaCoverSurface

    func raise() {
        generation += 1
        DockPresence.coverUp = true
        DockPresence.settle()                           // a regular app while the cover is up
        let reduced = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        var timeline = ShieldLookTimeline(shownAt: CACurrentMediaTime(), reduced: reduced)
        timeline.paused = !access.gate.mayCapture
        self.timeline = timeline
        state.dimmed = false
        buildPanels()
        panels.forEach { $0.orderFrontRegardless() }
        panels.first?.makeKey()
        setKiosk(true)
        AppWindowPresentation.coverTakesFront()
        looks.forEach { $0.enter(reduced: reduced) }
        drawFrame()                                     // the first frame (Reduce Motion: the only one)
        accessWatch = access.$gate.sink { [weak self] gate in
            MainActor.assumeIsolated { self?.setPaused(!gate.mayCapture) }
        }
        startWatching()
    }

    /// The look's dismiss (about 0.9 s, ShieldDismiss), then the cover goes; the watchers stop at once.
    func drop() {
        stopWatching()
        accessWatch = nil
        camera.stop()
        state.asking = false
        let reduced = timeline?.reduced ?? true
        timeline?.dismissedAt = CACurrentMediaTime()
        scheduleFrames()
        looks.forEach { $0.dismiss(reduced: reduced) }
        let raised = generation
        DispatchQueue.main.asyncAfter(deadline: .now() + ShieldDismiss.length(reduced: reduced)) { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.generation == raised else { return }
                self.panels.forEach { $0.orderOut(nil) }
                self.panels = []
                self.looks = []
                self.timeline = nil
                self.clickThrough = false
                self.setKiosk(false)
                DockPresence.coverUp = false
                DockPresence.settle()
            }
        }
    }

    func setClickThrough(_ on: Bool) -> [Int] {
        clickThrough = on
        panels.forEach { $0.passClicks(on) }
        return panels.map(\.windowNumber)
    }

    func takeFront() {
        guard !panels.isEmpty else { return }
        panels.forEach { $0.orderFrontRegardless() }
        panels.first?.makeKey()
        AppWindowPresentation.coverTakesFront()
        setKiosk(true)
    }

    /// APP-982: stopped means nothing is drawn any more (the frame clock stops), the last frame dimmed to near-black.
    func show(dimmed: Bool) {
        guard state.dimmed != dimmed else { return }
        state.dimmed = dimmed
        timeline?.stopped = dimmed
        looks.forEach { $0.setDimmed(dimmed, reduced: timeline?.reduced ?? true) }
        scheduleFrames()
    }

    func show(line: String) {
        if state.line != line { state.line = line }
    }

    func authenticate(_ done: @escaping @MainActor (Bool) -> Void) {
        state.asking = true
        panels.forEach { $0.level = ShieldPanel.askingLevel }
        LAContext().evaluatePolicy(.deviceOwnerAuthentication, localizedReason: InsomniaWords.unlockReason) { ok, _ in
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    self.panels.forEach { $0.level = ShieldPanel.coverLevel }
                    self.state.asking = false
                    done(ok)
                }
            }
        }
    }

    func startCamera() { camera.start() }
    func stopCamera() { camera.stop() }
    var cameraAccess: String { InsomniaCamera.access }

    // MARK: Windows

    private func buildPanels() {
        panels.forEach { $0.orderOut(nil) }
        looks = []
        panels = NSScreen.screens.enumerated().map { index, screen in
            let panel = ShieldPanel.make(frame: screen.frame)
            let look = ShieldLookInUse(size: screen.frame.size, scale: screen.backingScaleFactor, main: index == 0,
                                       state: state, firstName: firstName)
            looks.append(look)
            panel.contentView = look.view
            panel.setFrame(screen.frame, display: false)
            panel.passClicks(clickThrough)
            return panel
        }
    }

    // MARK: The frame clock (one for every display)

    private func setPaused(_ paused: Bool) {
        guard timeline != nil, timeline?.paused != paused else { return }
        timeline?.paused = paused
        if !paused { drawFrame() }
        scheduleFrames()
    }

    /// One frame on every display, then the clock set for what the look needs next.
    private func drawFrame() {
        guard let timeline else { return }
        let now = CACurrentMediaTime()
        looks.forEach { $0.draw(timeline, at: now) }
        scheduleFrames()
    }

    /// Write at 30 frames a second, Breathe at 20; none while still, paused, stopped or dismissing.
    private func scheduleFrames() {
        let wanted = timeline?.framesPerSecond(at: CACurrentMediaTime()) ?? 0
        guard wanted != framesPerSecond || (wanted > 0 && frames == nil) else { return }
        frames?.invalidate()
        frames = nil
        framesPerSecond = wanted
        guard wanted > 0 else { return }
        let timer = Timer(timeInterval: 1 / Double(wanted), repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.drawFrame() }
        }
        RunLoop.main.add(timer, forMode: .common)
        frames = timer
    }

    private func setKiosk(_ on: Bool) {
        let options: NSApplication.PresentationOptions = on ? Self.kiosk : []
        if NSApp.presentationOptions != options { NSApp.presentationOptions = options }
    }

    /// A display came or went: one cover per display again, as they were (click-through, the asking level).
    private func screensChanged() {
        guard !panels.isEmpty else { return }
        let level = panels.first?.level ?? ShieldPanel.coverLevel
        buildPanels()
        drawFrame()
        panels.forEach { $0.level = level; $0.orderFrontRegardless() }
        if !clickThrough { panels.first?.makeKey() }
        controller?.coverWindowsChanged(panels.map(\.windowNumber))     // the new numbers, for FERB's hit tests
    }

    // MARK: Watchers (only while the cover is up)

    private func startWatching() {
        stopWatching()
        startTap()
        let workspace = NSWorkspace.shared.notificationCenter
        observers.append((workspace, workspace.addObserver(forName: NSWorkspace.didActivateApplicationNotification,
                                                           object: nil, queue: .main) { [weak self] note in
            let app = note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication
            let bundle = app?.bundleIdentifier, name = app?.localizedName, pid = app?.processIdentifier ?? 0
            MainActor.assumeIsolated { self?.controller?.activated(bundle: bundle, name: name, pid: pid) }
        }))
        observers.append((NotificationCenter.default, NotificationCenter.default.addObserver(
            forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.screensChanged() }
        }))
        // One gate for every FERB window while the cover is up: no key reaches any of them (nobody types a request or
        // an answer under the cover), and no click reaches one beneath it. At the cover any click or key asks for
        // Touch ID (the prototype's way); everything else there wakes its look.
        monitor = NSEvent.addLocalMonitorForEvents(matching: Self.gatedEvents) { [weak self] event in
            let verdict = Self.gate(event.type, onCover: event.window is ShieldPanel)
            let passes = MainActor.assumeIsolated { () -> Bool in
                guard let self, !self.panels.isEmpty else { return true }
                if verdict.asks { self.controller?.unlock() }
                if verdict.touches { self.controller?.coverTouched() }
                return verdict.passes
            }
            return passes ? event : nil
        }
        timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.controller?.tick() }
        }
        timer.map { RunLoop.main.add($0, forMode: .common) }
    }

    private func stopWatching() {
        for (center, token) in observers { center.removeObserver(token) }
        observers = []
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
        timer?.invalidate()
        timer = nil
        stopTap()
    }

    nonisolated private static let keyEvents: Set<NSEvent.EventType> = [.keyDown, .keyUp, .flagsChanged]
    nonisolated private static let clicks: Set<NSEvent.EventType> = [.leftMouseDown, .rightMouseDown, .otherMouseDown]

    /// What the cover's one gate does with an event FERB.app gets while the cover is up: `passes` (only the cover's own
    /// pointer moves, scrolls and gestures reach a FERB window), `asks` (a key anywhere or a click on the cover asks
    /// for Touch ID), `touches` (anything at the cover but a key wakes its look).
    nonisolated static func gate(_ type: NSEvent.EventType, onCover: Bool) -> (passes: Bool, asks: Bool, touches: Bool) {
        let key = keyEvents.contains(type)
        let asks = type == .keyDown || (onCover && clicks.contains(type))
        let touches = onCover && !key
        return (touches && !asks, asks, touches)
    }
    private static let gatedEvents: NSEvent.EventTypeMask = [
        .keyDown, .keyUp, .flagsChanged, .leftMouseDown, .leftMouseUp, .rightMouseDown, .rightMouseUp, .otherMouseDown,
        .otherMouseUp, .leftMouseDragged, .rightMouseDragged, .otherMouseDragged, .scrollWheel, .mouseMoved, .magnify,
        .swipe, .rotate, .gesture,
    ]

    // MARK: The person-input tap

    /// Pointer, scroll and trackpad events (CGEventType raw values; gestures are 29 and up).
    static let pointerMask: CGEventMask = [1, 2, 3, 4, 5, 6, 7, 22, 25, 26, 27, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38]
        .reduce(0) { $0 | (1 << CGEventMask($1)) }
    /// Keys, modifiers and the system-defined keys (brightness, media).
    static let keyMask: CGEventMask = [10, 11, 12, 14].reduce(0) { $0 | (1 << CGEventMask($1)) }
    /// Keys only with FERB.app's Input Monitoring already granted; nothing here ever asks for it.
    static func tapMask(listenAccess: Bool) -> CGEventMask { listenAccess ? pointerMask | keyMask : pointerMask }

    /// A listen-only session tap: it sees events and changes none. Keys need FERB.app's Input Monitoring: without it
    /// the tap watches the pointer and the trackpad only, and nothing asks for the grant while the owner is away.
    private func startTap() {
        let callback: CGEventTapCallBack = { _, type, event, info in
            guard let info else { return Unmanaged.passUnretained(event) }
            let cover = Unmanaged<InsomniaCoverWindows>.fromOpaque(info).takeUnretainedValue()
            let raw = type.rawValue
            let pid = event.getIntegerValueField(.eventSourceUnixProcessID)
            let source = event.getIntegerValueField(.eventSourceStateID)
            MainActor.assumeIsolated {
                if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
                    cover.tap.map { CGEvent.tapEnable(tap: $0, enable: true) }
                } else {
                    cover.controller?.sawInput(eventType: raw, sourcePID: pid, sourceStateID: source)
                }
            }
            return Unmanaged.passUnretained(event)
        }
        let mask = Self.tapMask(listenAccess: CGPreflightListenEventAccess())
        guard let port = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .tailAppendEventTap, options: .listenOnly,
                                           eventsOfInterest: mask, callback: callback,
                                           userInfo: Unmanaged.passUnretained(self).toOpaque()) else {
            // No tap: nothing is reported from here; the daemon's status says what it knows.
            return
        }
        tap = port
        tapSource = CFMachPortCreateRunLoopSource(nil, port, 0)
        CFRunLoopAddSource(CFRunLoopGetMain(), tapSource, .commonModes)
        CGEvent.tapEnable(tap: port, enable: true)
    }

    private func stopTap() {
        if let tap { CGEvent.tapEnable(tap: tap, enable: false); CFMachPortInvalidate(tap) }
        if let tapSource { CFRunLoopRemoveSource(CFRunLoopGetMain(), tapSource, .commonModes) }
        tap = nil
        tapSource = nil
    }
}
