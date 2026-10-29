import AppKit

protocol MagneticActuator: AnyObject {
    func play(_ signal: MagneticHapticSignal) -> TimeInterval?
    func close()
}

/// All native resource operations share one serial queue. One finite segment is
/// outstanding at a time; display frames replace a mailbox, never enqueue tones.
final class TrackpadMagneticOutput: MagneticContinuousOutput, @unchecked Sendable {
    private final class State: @unchecked Sendable {
        let lock = NSLock()
        var signal: MagneticHapticSignal?
        var generation = 0
        var failed = false
        var actuator: MagneticActuator? // worker queue only
    }
    private let state = State()
    private let queue = DispatchQueue(label: "ferb.magnetic-haptics", qos: .userInteractive)
    private let makeActuator: () -> MagneticActuator?

    init(makeActuator: @escaping () -> MagneticActuator? = { NativeMagneticActuator() }) {
        self.makeActuator = makeActuator
    }
    var unavailable: Bool { state.lock.withLock { state.failed } }

    func update(_ signal: MagneticHapticSignal) {
        let state = self.state, queue = self.queue, factory = makeActuator
        let generation: Int? = state.lock.withLock {
            guard signal.amplitude.isFinite, signal.frequency.isFinite else { return nil }
            let start = state.signal == nil
            state.signal = signal
            if start { state.generation += 1; state.failed = false }
            return start ? state.generation : nil
        }
        guard let generation else { return }
        queue.async { Self.render(state, queue: queue, generation: generation, factory: factory) }
    }

    private static func render(_ state: State, queue: DispatchQueue, generation: Int,
                               factory: @escaping () -> MagneticActuator?) {
        guard state.lock.withLock({ state.generation == generation && state.signal != nil }) else { return }
        if state.actuator == nil { state.actuator = factory() }
        // Native calls never hold the mailbox lock: an unavailable device cannot
        // stall the UI. This snapshot commits one finite segment; cancellation
        // revokes every later segment and closes serially after this submission.
        let signal = state.lock.withLock { state.generation == generation ? state.signal : nil }
        guard let signal else { return }
        let started = DispatchTime.now()
        guard let duration = state.actuator?.play(signal), duration > 0, duration.isFinite else {
            state.lock.withLock { if state.generation == generation { state.failed = true } }
            state.actuator?.close(); state.actuator = nil
            return
        }
        queue.asyncAfter(deadline: started + duration) {
            render(state, queue: queue, generation: generation, factory: factory)
        }
    }

    func stop() {
        let state = self.state
        state.lock.withLock { state.generation += 1; state.signal = nil; state.failed = false }
        queue.async { state.actuator?.close(); state.actuator = nil }
    }
    deinit { stop() }
}

/// Private API declarations and parametric dictionary informed by Tactile (MIT);
/// see THIRD_PARTY_NOTICES. Everything is optional and resolved at runtime.
private final class NativeMagneticActuator: MagneticActuator {
    typealias Create = @convention(c) (UInt64) -> UnsafeMutableRawPointer?
    typealias OpenClose = @convention(c) (UnsafeMutableRawPointer) -> Int32
    typealias List = @convention(c) () -> Unmanaged<CFArray>?
    typealias DeviceID = @convention(c) (UnsafeMutableRawPointer, UnsafeMutablePointer<UInt64>) -> Int32
    typealias BuiltIn = @convention(c) (UnsafeMutableRawPointer) -> Bool
    typealias Waveform = @convention(c) (CFDictionary, Int32) -> UnsafeMutableRawPointer?
    typealias Play = @convention(c) (UnsafeMutableRawPointer, UnsafeMutableRawPointer, UInt32, Float, Float) -> Int32
    private let library: UnsafeMutableRawPointer
    private let closeActuator: OpenClose
    private let waveform: Waveform
    private let playWaveform: Play
    private var actuator: UnsafeMutableRawPointer?

    init?() {
        guard let library = dlopen("/System/Library/PrivateFrameworks/MultitouchSupport.framework/MultitouchSupport", RTLD_NOW) else { return nil }
        func symbol<T>(_ name: String, _: T.Type) -> T? {
            dlsym(library, name).map { unsafeBitCast($0, to: T.self) }
        }
        guard let create = symbol("MTActuatorCreateFromDeviceID", Create.self),
              let open = symbol("MTActuatorOpen", OpenClose.self),
              let close = symbol("MTActuatorClose", OpenClose.self),
              let waveform = symbol("MTActuationCreateFromDictionary", Waveform.self),
              let play = symbol("MTActuationActuate", Play.self),
              let list = symbol("MTDeviceCreateList", List.self),
              let deviceID = symbol("MTDeviceGetDeviceID", DeviceID.self) else {
            dlclose(library); return nil
        }
        let isBuiltIn = symbol("MTDeviceIsBuiltIn", BuiltIn.self)
        var ids: [UInt64] = []
        if let devices = list()?.takeRetainedValue() as? [AnyObject] {
            for device in devices {
                var id: UInt64 = 0
                let pointer = Unmanaged.passUnretained(device).toOpaque()
                if deviceID(pointer, &id) == 0, id != 0 {
                    if isBuiltIn?(pointer) == true { ids.insert(id, at: 0) }
                    else { ids.append(id) }
                }
            }
        }
        ids.append(0x200000001)
        var opened: UnsafeMutableRawPointer?
        for id in ids {
            guard let candidate = create(id) else { continue }
            if open(candidate) == 0 { opened = candidate; break }
            Unmanaged<AnyObject>.fromOpaque(candidate).release()
        }
        guard let opened else { dlclose(library); return nil }
        self.library = library; closeActuator = close; self.waveform = waveform; playWaveform = play
        actuator = opened
    }

    func play(_ signal: MagneticHapticSignal) -> TimeInterval? {
        guard let actuator else { return nil }
        // One whole cycle at 100–125 Hz is 192–240 ticks on the 24 kHz
        // firmware grid, inside the upstream-reported 192–255 tick window.
        let period = max(1, Int((24_000 / max(100, min(125, signal.frequency))).rounded()))
        let ticks = period * max(1, 255 / period)
        let milliseconds = Double(ticks) / 24
        let shape: [String: Any] = [
            "BaseWaveform": ["Type": "Gaussian", "Amplitude": 0.0, "DurationMS": milliseconds],
            "Tones": [["Type": "Sine", "Amplitude": 1.0, "DurationMS": milliseconds,
                       "DelayMS": 0.0, "FrequencykHz": 24.0 / Double(period)]]
        ]
        guard let wave = waveform(shape as CFDictionary, 0) else { return nil }
        defer { Unmanaged<AnyObject>.fromOpaque(wave).release() }
        guard playWaveform(wave, actuator, 0, Float(max(0, min(1, signal.amplitude))), 0) == 0 else { return nil }
        return Double(ticks) / 24_000
    }

    func close() {
        guard let actuator else { return }
        _ = closeActuator(actuator)
        Unmanaged<AnyObject>.fromOpaque(actuator).release()
        self.actuator = nil
    }
    deinit { close(); dlclose(library) }
}
