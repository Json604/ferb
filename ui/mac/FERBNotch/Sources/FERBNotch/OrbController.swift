import AppKit
import QuartzCore
import NotchCore

/// The orb the intro drives (Task 8 takes this, so its tests can use a fake).
@MainActor
protocol OrbControlling: AnyObject {
    /// Drips off the notch and flies to `target` (AppKit global coordinates); returns once it has arrived.
    func launch(to target: CGPoint, instruction: IntroLine?) async
    /// Docks beside System Settings' window, holding `url` (the agent app) as a draggable icon, and follows it.
    func dockBesideSettings(dragging url: URL)
    /// The small happy hop when a step succeeds.
    func hop()
    /// Flies back and melts into the notch; returns once it is home.
    func flyHome() async
    /// Everything stops at once and the orb is gone; safe mid-flight and safe twice.
    func tearDown()
}

/// What draws the orb (OrbPanel), in AppKit global coordinates.
@MainActor
protocol OrbSurface: AnyObject {
    var isVisible: Bool { get }
    func place(center: CGPoint)
    func setAppearance(scale: CGFloat, opacity: CGFloat)
    func setInstruction(_ line: IntroLine?)
    func setDragItem(_ url: URL?)
    func setWaiting(_ waiting: Bool)
    func show()
    func hide()
}

/// Frame ticks with the time since the last one, in seconds.
@MainActor
protocol OrbClock: AnyObject {
    func start(_ tick: @escaping (Double) -> Void)
    func stop()
}

@MainActor
protocol SettingsWindowTracking: AnyObject {
    var onChange: ((CGPoint?) -> Void)? { get set }
    func start()
    func stop()
}

extension SettingsWindowTracker: SettingsWindowTracking {}

/// The orb's choreography: OrbMotion's springs at display refresh, retargetable mid-flight, with the family's orb
/// sounds on its keyframes (drip-off as it sets off, hop, melt-back on arrival at its target and at home; nothing
/// while it flies or lands). Reduce Motion: it fades in beside its target and fades out in place instead of flying.
@MainActor
final class OrbController: OrbControlling {
    nonisolated static let size = CGSize(width: 56, height: 56)
    static let fadeSeconds = 0.25
    /// Decorative motion never runs longer than this; a flight still under way then lands where it was going.
    static let maxFlightSeconds = 2.0
    static let hopSeconds = 0.36
    static let hopHeight: CGFloat = 12
    /// The orb's size as it leaves or rejoins the notch (a quick scale and fade stands in for a liquid split).
    static let dripScale: CGFloat = 0.35

    private enum Arrival {
        case target(IntroLine?)   // melt-back, show the instruction, release the caller
        case follow               // following Settings' window: silent
        case home                 // melt-back and into the notch
    }
    private struct Flight {
        var target: CGPoint
        var elapsed = 0.0
        var arrival: Arrival
    }
    private struct Fade {
        var from: CGFloat
        var to: CGFloat
        var scales: Bool
        var elapsed = 0.0
        var then: (() -> Void)?
    }

    private let surface: OrbSurface
    private let sound: FerbSoundPlaying
    private let clock: OrbClock
    private let home: @MainActor () -> CGPoint
    private let reduceMotion: @MainActor () -> Bool
    private let makeTracker: @MainActor () -> SettingsWindowTracking
    private var motion = OrbMotion(origin: .zero)
    private var position = CGPoint.zero
    private var appearance: CGFloat = 0
    private var flight: Flight?
    private var fade: Fade?
    private var hopElapsed: Double?
    private var waiter: CheckedContinuation<Void, Never>?
    private var tracker: SettingsWindowTracking?
    /// Away from the notch: on screen, or hidden while Settings isn't in front.
    private var out = false
    private var hiddenForDock = false

    init(surface: OrbSurface? = nil, sound: FerbSoundPlaying, clock: OrbClock? = nil,
         home: @escaping @MainActor () -> CGPoint = { OrbController.notchHome() },
         reduceMotion: @escaping @MainActor () -> Bool = { NSWorkspace.shared.accessibilityDisplayShouldReduceMotion },
         makeTracker: @escaping @MainActor () -> SettingsWindowTracking = { SettingsWindowTracker(orb: OrbController.size) }) {
        self.surface = surface ?? OrbPanel()
        self.sound = sound
        self.clock = clock ?? DisplayLinkClock()
        self.home = home
        self.reduceMotion = reduceMotion
        self.makeTracker = makeTracker
    }

    func launch(to target: CGPoint, instruction: IntroLine?) async {
        release()                                   // a new flight replaces one still under way
        surface.setInstruction(nil)
        if reduceMotion() {
            out = true
            sound.play(.orbMelt, at: target)
            surface.setInstruction(instruction)
            await withCheckedContinuation { continuation in
                waiter = continuation
                reappear(at: target) { [weak self] in self?.release() }
            }
            return
        }
        if !out { setOff() } else if hiddenForDock || !surface.isVisible { reappear(at: position) }
        await withCheckedContinuation { continuation in
            waiter = continuation
            fly(to: target, spec: .open, arrival: .target(instruction))
        }
    }

    func dockBesideSettings(dragging url: URL) {
        surface.setDragItem(url)
        surface.setWaiting(true)
        stopTracking()
        let t = makeTracker()
        tracker = t
        t.onChange = { [weak self] point in self?.settingsMoved(point) }
        t.start()
    }

    func hop() {
        guard out, !hiddenForDock, surface.isVisible else { return }
        sound.play(.orbHop, at: position)
        guard !reduceMotion() else { return }
        hopElapsed = 0
        startClock()
    }

    func flyHome() async {
        stopTracking()
        surface.setDragItem(nil)
        surface.setWaiting(false)
        surface.setInstruction(nil)
        release()
        guard out else { return }
        guard !hiddenForDock, surface.isVisible else { return finishHome() }   // nothing on screen to bring back
        if reduceMotion() {
            flight = nil
            sound.play(.orbMelt, at: home())
            await withCheckedContinuation { continuation in
                waiter = continuation
                beginFade(to: 0, scales: false) { [weak self] in self?.finishHome() }
            }
            return
        }
        await withCheckedContinuation { continuation in
            waiter = continuation
            fly(to: home(), spec: .close, arrival: .home)
        }
    }

    func tearDown() {
        stopTracking()
        clock.stop()
        flight = nil
        fade = nil
        hopElapsed = nil
        sound.stopAll()
        surface.setDragItem(nil)
        surface.setInstruction(nil)
        surface.setWaiting(false)
        surface.hide()
        out = false
        hiddenForDock = false
        release()
    }

    // MARK: - Motion

    private func settingsMoved(_ point: CGPoint?) {
        guard let point else {
            hiddenForDock = true
            flight = nil
            fade = nil
            hopElapsed = nil
            clock.stop()
            surface.hide()
            release()                                // a launch on its way there isn't left waiting
            return
        }
        if !out {
            if reduceMotion() {
                out = true
                reappear(at: point)
                sound.play(.orbMelt, at: point)
            } else {
                setOff()
                fly(to: point, spec: .open, arrival: .target(nil))
            }
        } else if hiddenForDock || !surface.isVisible {
            reappear(at: point)
        } else if reduceMotion() {
            position = point
            motion = OrbMotion(origin: point)
            surface.place(center: point)
        } else {
            // A launch still on its way keeps its arrival (its melt-back and its caller) at the new point.
            fly(to: point, spec: .open, arrival: flight?.arrival ?? .follow)
        }
    }

    /// Leaves the notch: small and clear at its centre, growing as it goes.
    private func setOff() {
        position = home()
        motion = OrbMotion(origin: position)
        out = true
        hiddenForDock = false
        appearance = 0
        surface.setAppearance(scale: Self.dripScale, opacity: 0)
        surface.place(center: position)
        surface.show()
        sound.play(.orbDrip, at: position)
        beginFade(to: 1, scales: true)
    }

    /// Fades in at a point without flying (Reduce Motion, or coming back beside Settings).
    private func reappear(at point: CGPoint, then: (() -> Void)? = nil) {
        position = point
        motion = OrbMotion(origin: point)
        flight = nil
        hiddenForDock = false
        appearance = 0
        surface.setAppearance(scale: 1, opacity: 0)
        surface.place(center: point)
        surface.show()
        beginFade(to: 1, scales: false, then: then)
    }

    private func fly(to target: CGPoint, spec: SpringSpec, arrival: Arrival) {
        motion.retarget(target, spec: spec)
        flight = Flight(target: target, arrival: arrival)
        startClock()
    }

    private func beginFade(to value: CGFloat, scales: Bool, then: (() -> Void)? = nil) {
        fade = Fade(from: appearance, to: value, scales: scales, then: then)
        startClock()
    }

    private func arrive(_ arrival: Arrival) {
        switch arrival {
        case .target(let instruction):
            sound.play(.orbMelt, at: position)
            if let instruction { surface.setInstruction(instruction) }
            release()
        case .follow:
            break
        case .home:
            sound.play(.orbMelt, at: position)
            beginFade(to: 0, scales: true) { [weak self] in self?.finishHome() }
        }
    }

    private func finishHome() {
        flight = nil
        fade = nil
        hopElapsed = nil
        surface.hide()
        out = false
        hiddenForDock = false
        release()
    }

    private func startClock() {
        clock.start { [weak self] dt in self?.tick(dt) }
    }

    private func tick(_ dt: Double) {
        if var f = fade {
            f.elapsed += dt
            let p = CGFloat(min(f.elapsed / Self.fadeSeconds, 1))
            appearance = f.from + (f.to - f.from) * p * p * (3 - 2 * p)
            surface.setAppearance(scale: f.scales ? Self.dripScale + (1 - Self.dripScale) * appearance : 1,
                                  opacity: appearance)
            fade = p < 1 ? f : nil
            if p >= 1 { f.then?() }
        }
        if var f = flight {
            position = motion.step(dt: dt)
            f.elapsed += dt
            if motion.settled || f.elapsed >= Self.maxFlightSeconds {
                position = f.target                  // settled doesn't snap; land exactly
                motion = OrbMotion(origin: f.target)
                flight = nil
                arrive(f.arrival)
            } else {
                flight = f
            }
        }
        var lift: CGFloat = 0
        if let h = hopElapsed {
            let t = h + dt
            hopElapsed = t < Self.hopSeconds ? t : nil
            if t < Self.hopSeconds { lift = Self.hopHeight * sin(.pi * t / Self.hopSeconds) }
        }
        if out { surface.place(center: CGPoint(x: position.x, y: position.y + lift)) }
        if fade == nil && flight == nil && hopElapsed == nil { clock.stop() }
    }

    private func stopTracking() {
        tracker?.onChange = nil
        tracker?.stop()
        tracker = nil
    }

    private func release() {
        let w = waiter
        waiter = nil
        w?.resume()
    }

    // MARK: - Where home is

    /// The notch the orb leaves from: the display with a camera housing, else the main display's virtual notch.
    static func notchHome() -> CGPoint {
        guard let screen = NSScreen.screens.first(where: { $0.safeAreaInsets.top > 0 }) ?? NSScreen.main
                ?? NSScreen.screens.first else { return .zero }
        return notchCentre(of: screen)
    }

    static func notchCentre(of screen: NSScreen) -> CGPoint {
        let menuBar = screen.frame.maxY - screen.visibleFrame.maxY
        let geometry = NotchGeometry(screen: screen.frame, topInset: screen.safeAreaInsets.top,
                                     leftAreaMaxX: screen.auxiliaryTopLeftArea?.maxX,
                                     rightAreaMinX: screen.auxiliaryTopRightArea?.minX, menuBarHeight: menuBar)
        return OrbOrigin.notchCentre(screenFrame: screen.frame, cutout: geometry.cutoutFrame, menuBarHeight: menuBar)
    }
}

/// The display's refresh (CADisplayLink from the screen), as NotchPresentation drives its spring.
@MainActor
final class DisplayLinkClock: NSObject, OrbClock {
    private var link: CADisplayLink?
    private var last: CFTimeInterval?
    private var tick: ((Double) -> Void)?

    func start(_ tick: @escaping (Double) -> Void) {
        self.tick = tick
        guard link == nil, let screen = NSScreen.main ?? NSScreen.screens.first else { return }
        let l = screen.displayLink(target: self, selector: #selector(step(_:)))
        l.add(to: .main, forMode: .common)
        link = l
        last = nil
    }

    func stop() {
        link?.invalidate()
        link = nil
        last = nil
        tick = nil
    }

    @objc private func step(_ l: CADisplayLink) {
        let now = l.targetTimestamp
        let dt = last.map { now - $0 } ?? (l.targetTimestamp - l.timestamp)
        last = now
        tick?(min(max(dt, 0), 1.0 / 20))            // a stalled frame doesn't jump the orb
    }
}
