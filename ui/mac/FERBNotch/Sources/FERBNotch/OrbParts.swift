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
