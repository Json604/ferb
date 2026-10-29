import AppKit
import Combine

struct MagneticHapticSignal: Equatable {
    let amplitude: Double
    let frequency: Double
}

protocol MagneticContinuousOutput: AnyObject {
    var unavailable: Bool { get }
    func update(_ signal: MagneticHapticSignal)
    func stop()
}

/// Uses the same proximity and fusion samples as the drawn field; no second clock.
@MainActor final class MagneticDockingHaptics {
    private let output: MagneticContinuousOutput
    private let tap: (NSHapticFeedbackManager.FeedbackPattern) -> Void
    private var observation: AnyCancellable?
    private var lastTime = -Double.infinity
    private var rejectedThrough = -Double.infinity
    private var armed = false
    private var lastTap = -Double.infinity
    var enabled = true { didSet { if !enabled { cancel() } } }
    var outputEnabled = true { didSet { if !outputEnabled { cancel() } } }

    init(output: MagneticContinuousOutput = TrackpadMagneticOutput(),
         tap: @escaping (NSHapticFeedbackManager.FeedbackPattern) -> Void = {
             NSHapticFeedbackManager.defaultPerformer.perform($0, performanceTime: .now)
         }) {
        self.output = output
        self.tap = tap
    }

    convenience init(feedback: SoundFeedback) {
        self.init()
        enabled = feedback.magneticHapticsEnabled
        observation = feedback.$magneticHapticsEnabled.sink { [weak self] in self?.enabled = $0 }
    }

    func update(_ sample: MagneticDocking.Sample) {
        guard sample.time.isFinite, sample.time >= lastTime, sample.time > rejectedThrough else { return }
        lastTime = sample.time
        guard enabled, outputEnabled else { return }
        if sample.completed {
            output.stop()
            if armed { tap(.alignment) }
            armed = false
            return
        }
        let active = sample.phase == .dragging || sample.phase == .fusing
        guard active else { cancel(); return }
        armed = true
        guard sample.proximity > 0 else { output.stop(); lastTap = -.infinity; return }
        let proximity = min(1, max(0, sample.proximity))
        let fusion = sample.phase == .fusing ? min(1, max(0, sample.fusionProgress)) : 0
        output.update(MagneticHapticSignal(amplitude: (0.145 * proximity) * (1 - 0.6 * fusion),
                                          frequency: 100 + 20 * proximity + 5 * fusion))
        // The supported fallback is discrete feedback, never advertised as continuous.
        if output.unavailable, sample.time - lastTap >= 0.32 - 0.20 * proximity {
            tap(.alignment)
            lastTap = sample.time
        }
    }

    func cancel() {
        rejectedThrough = lastTime
        armed = false
        lastTap = -.infinity
        output.stop()
    }

    func shutdown() { cancel() }
}
