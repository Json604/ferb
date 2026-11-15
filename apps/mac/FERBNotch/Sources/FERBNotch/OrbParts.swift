import AppKit
import QuartzCore
import NotchCore

/// The orb the intro drives (Task 8 takes this, so its tests can use a fake).
@MainActor
protocol OrbControlling: AnyObject {
    /// Intermediate returns only clear the guide; one homecoming happens when the tour ends.
    var holdsHomecoming: Bool { get set }
    /// Drips off the notch and flies to `target` (AppKit global coordinates); returns once it has arrived.
    func launch(to target: CGPoint, instruction: IntroLine?) async
    /// Docks beside System Settings' window, holding `url` (the agent app) as a draggable icon (nil: nothing to drag,
    /// as in Login Items), with `instruction` in its bubble, and follows it. `showWay`: a faded copy first shows the
    /// drag into Settings' list (APP-639); not once the drop was accepted.
    func dockBesideSettings(dragging urls: [URL], instruction: IntroLine?, showWay: Bool)
    /// The small happy hop when a step succeeds.
    func hop()
    /// Flies back and melts into the notch; returns once it is home.
    func flyHome() async
    /// Its motion, tracking and waits stop at once and the orb is gone; safe mid-flight and safe twice. Sounds already
    /// playing are left to the caller (IntroController.end), since the engine is shared with everyday sounds.
    func tearDown()
    /// The orb has melted back into the notch (the intro swells the notch once: NotchModel.pulseIntro).
    var onHomeArrival: (@MainActor () -> Void)? { get set }
    /// The drag destination accepted the app; this is not evidence of a permission grant.
    var onDropAccepted: (() -> Void)? { get set }
}

extension OrbControlling {
    var holdsHomecoming: Bool { get { false } set {} }
    var onDropAccepted: (() -> Void)? { get { nil } set {} }
    func dockBesideSettings(dragging url: URL) { dockBesideSettings(dragging: url, instruction: nil) }
    func dockBesideSettings(dragging url: URL?, instruction: IntroLine?) {
        dockBesideSettings(dragging: url, instruction: instruction, showWay: url != nil)
    }
    func dockBesideSettings(dragging url: URL?, instruction: IntroLine?, showWay: Bool) {
        dockBesideSettings(dragging: url.map { [$0] } ?? [], instruction: instruction, showWay: showWay)
    }
}

/// What draws the orb (OrbPanel), in AppKit global coordinates.
@MainActor
protocol OrbSurface: AnyObject {
    var onDropAccepted: (() -> Void)? { get set }
    var isVisible: Bool { get }
    func place(center: CGPoint)
    func setAppearance(scale: CGFloat, opacity: CGFloat)
    func setInstruction(_ line: IntroLine?)
    func setDragItem(_ url: URL?)
    /// Several items in one drag (Full Disk Access: the agent app and FERB.app, APP-646); the first one's icon shows.
    func setDragItems(_ urls: [URL])
    func setWaiting(_ waiting: Bool)
    func show()
    func hide()
    /// A faded copy of the dragged logo shows the way to `point` (System Settings' list), over and over, until nil
    /// (APP-639).
    func demonstrateDrag(to point: CGPoint?)
}

extension OrbSurface {
    var onDropAccepted: (() -> Void)? { get { nil } set {} }
    func setDragItems(_ urls: [URL]) { setDragItem(urls.first) }
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
    /// System Settings' window as last reported (AppKit coordinates); nil while hidden.
    var settingsFrame: CGRect? { get }
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
