import Foundation
import CoreGraphics
@testable import FERBNotch
@testable import NotchCore

@MainActor
final class FakeOrbSurface: OrbSurface {
    var visible = false
    var center = CGPoint.zero
    var opacity: CGFloat = 0
    var dragItem: URL?
    var instruction: IntroLine?
    var isVisible: Bool { visible }
    func place(center: CGPoint) { self.center = center }
    func setAppearance(scale: CGFloat, opacity: CGFloat) { self.opacity = opacity }
    func setInstruction(_ line: IntroLine?) { instruction = line }
    func setDragItem(_ url: URL?) { dragItem = url }
    func setWaiting(_ waiting: Bool) {}
    func show() { visible = true }
    func hide() { visible = false }
}

/// Frame ticks on demand; `now` is the time the ticks add up to.
@MainActor
final class ManualClock: OrbClock {
    private var tick: ((Double) -> Void)?
    private(set) var now = 0.0
    var running: Bool { tick != nil }
    func start(_ tick: @escaping (Double) -> Void) { self.tick = tick }
    func stop() { tick = nil }
    func advance(_ dt: Double) {
        now += dt
        tick?(dt)
    }
}

/// Records each sound with the clock's time when it was played.
@MainActor
final class FakeSoundPlayer: FerbSoundPlaying {
    var played: [FerbSound] = []
    var times: [(FerbSound, Double)] = []
    var stops = 0
    var warm: [Bool] = []
    var outputLatency: TimeInterval = 0
    weak var clock: ManualClock?
    func play(_ s: FerbSound, at point: CGPoint?) {
        played.append(s)
        times.append((s, clock?.now ?? 0))
    }
    func stopAll() { stops += 1 }
    func keepWarm(_ on: Bool) { warm.append(on) }
}

@MainActor
final class FakeTracker: SettingsWindowTracking {
    var onChange: ((CGPoint?) -> Void)?
    var running = false
    func start() { running = true }
    func stop() { running = false }
}
