import AppKit
import QuartzCore
import NotchCore

/// The orb's choreography: OrbMotion's springs at display refresh, retargetable mid-flight, with the family's orb
/// sounds on its keyframes (drip-off as it sets off, hop, melt-back on arrival at its target and at home; nothing
/// while it flies or lands). Reduce Motion: it fades in beside its target and fades out in place instead of flying.
///
/// Each keyframe sound is issued the output's latency before its keyframe shows, so it is heard as the orb moves
/// (Bluetooth adds well over 100 ms). An arrival is foreseen: the spring is solved that far ahead (its closed form
/// steps any interval exactly) and the melt is issued when it will have landed by then. A departure or a hop can't
/// be foreseen, so its sound is issued at once and the motion waits that long before it starts.
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
    /// The longest lead a sound gets over its keyframe.
    static let maxLead = 0.5

    private enum Arrival {
        case target(IntroLine?)   // melt-back, show the instruction, release the caller
        case follow               // following Settings' window: silent
        case home                 // melt-back and into the notch

        var melts: Bool {
            if case .follow = self { return false }
            return true
        }
    }
    private struct Flight {
        var target: CGPoint
        var elapsed = 0.0
        var arrival: Arrival
        var meltIssued = false
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
    /// The motion waits this long for a departure's sound to reach the speakers.
    private var hold = 0.0
    private var waiter: CheckedContinuation<Void, Never>?
    private var tracker: SettingsWindowTracking?
    /// The current dock shows the way to drag the logo (APP-639).
    private var showsWay = false
    /// Away from the notch: on screen, or hidden while Settings isn't in front.
    private var out = false
    var holdsHomecoming = false
    private var hiddenForDock = false
    /// The orb has melted back into the notch (Task 8 swells the notch once: NotchModel.pulseIntro).
    var onDropAccepted: (() -> Void)? {
        get { surface.onDropAccepted }
        set { surface.onDropAccepted = newValue }
    }
    var onHomeArrival: (@MainActor () -> Void)?
    private var lead: Double { min(max(sound.outputLatency, 0), Self.maxLead) }

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

    deinit {
        waiter?.resume()                             // a caller never waits on an orb that is gone
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
                hold = lead
            }
            return
        }
        if !out { setOff() } else if hiddenForDock || !surface.isVisible { reappear(at: position) }
        await withCheckedContinuation { continuation in
            waiter = continuation
            fly(to: target, spec: .open, arrival: .target(instruction))
        }
    }

    func dockBesideSettings(dragging urls: [URL], instruction: IntroLine?, showWay: Bool) {
        showsWay = showWay && !urls.isEmpty && !reduceMotion()
        surface.demonstrateDrag(to: nil)
        surface.setDragItems(urls)
        surface.setInstruction(instruction)
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
        hopElapsed = -lead                           // the lift starts as the sound is heard
        startClock()
    }

    func flyHome() async {
        stopTracking()
        surface.setDragItem(nil)
        surface.setWaiting(false)
        surface.setInstruction(nil)
        release()
        guard out, !holdsHomecoming else { return }
        guard !hiddenForDock, surface.isVisible else { return finishHome() }   // nothing on screen to bring back
        if reduceMotion() {
            flight = nil
            sound.play(.orbMelt, at: home())
            await withCheckedContinuation { continuation in
                waiter = continuation
                beginFade(to: 0, scales: false) { [weak self] in
                    self?.onHomeArrival?()
                    self?.finishHome()
                }
                hold = lead
            }
            return
        }
        await withCheckedContinuation { continuation in
            waiter = continuation
            fly(to: home(), spec: .close, arrival: .home)
        }
    }

    /// Stops only what the orb schedules (motion, tracking, its panel, its waiters). The sound engine is shared with
    /// everyday sounds, so what is already playing is IntroController.end's to stop (a skip) or let ring (the end).
    func tearDown() {
        onDropAccepted = nil
        holdsHomecoming = false
        stopTracking()
        clock.stop()
        flight = nil
        fade = nil
        hopElapsed = nil
        hold = 0
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
            hold = 0
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
                hold = lead
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
        hold = lead
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

    private func arrive(_ arrival: Arrival, meltIssued: Bool) {
        switch arrival {
        case .target(let instruction):
            if !meltIssued { sound.play(.orbMelt, at: position) }
            if let instruction { surface.setInstruction(instruction) }
            release()
            showWay()
        case .follow:
            showWay()
        case .home:
            if !meltIssued { sound.play(.orbMelt, at: position) }
            onHomeArrival?()
            beginFade(to: 0, scales: true) { [weak self] in self?.finishHome() }
        }
    }

    /// Beside System Settings with something to drag: the faded copy goes from the logo into Settings' list (its right
    /// pane, two thirds across, halfway down), and again from wherever the logo is after Settings moves.
    private func showWay() {
        guard showsWay, tracker != nil, let frame = tracker?.settingsFrame else { return }
        surface.demonstrateDrag(to: CGPoint(x: frame.minX + frame.width * 2 / 3, y: frame.midY))
    }

    private func finishHome() {
        flight = nil
        fade = nil
        hopElapsed = nil
        hold = 0
        surface.hide()
        out = false
        hiddenForDock = false
        release()
    }

    private func startClock() {
        clock.start { [weak self] dt in self?.tick(dt) }
    }

    private func tick(_ elapsed: Double) {
        var dt = elapsed
        if hold > 0 {
            let waited = min(hold, dt)
            hold -= waited
            dt -= waited
        }
        if dt > 0, var f = fade {
            f.elapsed += dt
            let p = CGFloat(min(f.elapsed / Self.fadeSeconds, 1))
            appearance = f.from + (f.to - f.from) * p * p * (3 - 2 * p)
            surface.setAppearance(scale: f.scales ? Self.dripScale + (1 - Self.dripScale) * appearance : 1,
                                  opacity: appearance)
            fade = p < 1 ? f : nil
            if p >= 1 { f.then?() }
        }
        if dt > 0, var f = flight {
            position = motion.step(dt: dt)
            f.elapsed += dt
            if motion.settled || f.elapsed >= Self.maxFlightSeconds {
                position = f.target                  // settled doesn't snap; land exactly
                motion = OrbMotion(origin: f.target)
                flight = nil
                arrive(f.arrival, meltIssued: f.meltIssued)
            } else {
                if !f.meltIssued, lead > 0, f.arrival.melts {
                    var ahead = motion
                    _ = ahead.step(dt: lead)
                    if ahead.settled || f.elapsed + lead >= Self.maxFlightSeconds {
                        sound.play(.orbMelt, at: f.target)
                        f.meltIssued = true
                    }
                }
                flight = f
            }
        }
        // A hop counts from its own call (its sound went out then), never from the end of a departure's hold.
        var lift: CGFloat = 0
        if let h = hopElapsed {
            let t = h + elapsed
            hopElapsed = t < Self.hopSeconds ? t : nil
            if t > 0 && t < Self.hopSeconds { lift = Self.hopHeight * sin(.pi * t / Self.hopSeconds) }
        }
        if out { surface.place(center: CGPoint(x: position.x, y: position.y + lift)) }
        if fade == nil && flight == nil && hopElapsed == nil && hold <= 0 { clock.stop() }
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
