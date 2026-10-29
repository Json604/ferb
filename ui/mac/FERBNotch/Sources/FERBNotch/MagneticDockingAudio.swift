import AVFAudio
import AudioCapture
import Combine

/// The source block owns this immutable handle until AVFAudio releases the block,
/// including when an exceptional teardown leaves the node attached to its graph.
private final class MagneticSynthStorage {
    let pointer = FERBMagneticSynthCreate(48_000)!
    deinit { FERBMagneticSynthDestroy(pointer) }
}

/// Output only. The callback captures only C state and renders through C atomics;
/// it never reads views, preferences, or mutable Swift state on the audio thread.
@MainActor
final class MagneticDockingAudio {
    private let engine = AVAudioEngine()
    private var source: AVAudioSourceNode?
    private let storage = MagneticSynthStorage()
    private var synth: OpaquePointer { storage.pointer }
    var outputEnabled = true
    private let startEngine: (AVAudioEngine) throws -> Void
    private let feedback: SoundFeedback
    private var observation: AnyCancellable?
    private var routeObserver: NSObjectProtocol?
    var hasPendingStop: Bool { stopTask != nil }
    private var stopTask: Task<Void, Never>?
    private var latest = MagneticDocking.Sample()
    private var completionTime: TimeInterval?
    private(set) var startError: Error?

    init(feedback: SoundFeedback, startEngine: @escaping (AVAudioEngine) throws -> Void = { if let error = FERBStartMagneticOutput($0) { throw error } }) {
        self.feedback = feedback
        self.startEngine = startEngine
        observation = feedback.objectWillChange.sink { [weak self] in
            DispatchQueue.main.async { self?.applyControls() }
        }
        routeObserver = NotificationCenter.default.addObserver(forName: .AVAudioEngineConfigurationChange,
            object: engine, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated {
                    self?.outputRouteChanged()
                }
            }
    }

    func outputRouteChanged() {
        FERBStopMagneticOutput(engine)
        if latest.phase == .dragging || latest.phase == .fusing { applyControls() }
    }

    func update(_ sample: MagneticDocking.Sample) {
        latest = sample
        let completion = sample.completed && completionTime != sample.time
        if completion { completionTime = sample.time }
        applyControls(completion: completion)
    }

    private func applyControls(completion: Bool = false) {
        let active = latest.phase == .dragging || latest.phase == .fusing
        let gain = feedback.magneticEnabled ? feedback.playbackVolume : 0
        let resonance = latest.phase == .fusing
            ? Float(latest.proximity * (1 - 0.35 * latest.fusionProgress)) : Float(latest.proximity)
        FERBMagneticSynthSet(synth, active ? resonance : 0, gain,
                             Float(latest.direction.x), completion && gain > 0)
        if (active && latest.proximity > 0) || completion {
            guard gain > 0 else { scheduleStop(); return }
            stopTask?.cancel()
            stopTask = nil
            startIfNeeded()
            if completion { scheduleStop() }
        } else { scheduleStop() }
    }

    private func startIfNeeded() {
        guard outputEnabled, !engine.isRunning else { return }
        if source == nil {
            let state = storage
            let node = AVAudioSourceNode { _, _, frames, list in
                let buffers = UnsafeMutableAudioBufferListPointer(list)
                guard buffers.count >= 2,
                      let l = buffers[0].mData?.assumingMemoryBound(to: Float.self),
                      let r = buffers[1].mData?.assumingMemoryBound(to: Float.self) else { return noErr }
                FERBMagneticSynthRender(state.pointer, l, r, frames)
                return noErr
            }
            if let error = FERBConfigureMagneticOutput(engine, node) {
                FERBDetachMagneticOutput(engine, node)
                startError = error
                return
            }
            source = node
        }
        do { try startEngine(engine); startError = nil } catch { startError = error }
    }

    private func scheduleStop() {
        guard stopTask == nil else { return }
        stopTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(300))
            guard !Task.isCancelled, let self else { return }
            FERBStopMagneticOutput(self.engine)
            self.stopTask = nil
        }
    }

    func cancel() {
        latest = MagneticDocking.Sample()
        FERBMagneticSynthCancel(synth)
        scheduleStop()
    }

    deinit {
        stopTask?.cancel()
        if let routeObserver { NotificationCenter.default.removeObserver(routeObserver) }
        FERBStopMagneticOutput(engine)
        if let source { FERBDetachMagneticOutput(engine, source) }
    }
}
