import AppKit
import AVFAudio
import AudioCapture
import NotchCore

/// What plays FERB's sounds; the intro's controller and its tests take this, not the engine.
@MainActor
protocol FerbSoundPlaying: AnyObject {
    /// `point` is in AppKit global coordinates; nil plays it centred at the notch.
    func play(_ s: FerbSound, at point: CGPoint?)
    /// Everything stops at once, the engine too, and keeping warm ends.
    func stopAll()
    /// On (the whole tour): the engine stays running between sounds instead of stopping when idle.
    func keepWarm(_ on: Bool)
    /// How long a sound takes from `play` to leaving the current output device, in seconds.
    var outputLatency: TimeInterval { get }
}

/// Where a sound sits: stereo position, the level drop and how far from the notch (0 at it, 1 a screen's height away).
struct SoundPlacement: Equatable {
    var pan: Float = 0
    var gainDB: Float = 0
    var distance: CGFloat = 0

    static func of(_ point: CGPoint?, screens: [(frame: CGRect, notch: CGPoint)]) -> SoundPlacement {
        guard let point, let screen = screens.first(where: { $0.frame.contains(point) })
                ?? screens.min(by: { hypot($0.frame.midX - point.x, $0.frame.midY - point.y)
                                     < hypot($1.frame.midX - point.x, $1.frame.midY - point.y) }),
              screen.frame.height > 0 else { return SoundPlacement() }
        let d = hypot(point.x - screen.notch.x, point.y - screen.notch.y)
        return SoundPlacement(pan: SoundPolicy.pan(x: point.x - screen.frame.minX, screenWidth: screen.frame.width),
                              gainDB: SoundPolicy.distanceGainDB(fromNotch: d, screenHeight: screen.frame.height),
                              distance: min(max(d / screen.frame.height, 0), 1))
    }
}

/// The decided family through one AVAudioEngine. Each dry mono source is placed where FERB is (the notch, or the orb)
/// and played with its room: the room's response is a true convolution of the source with render.py's impulse
/// response, computed once per file, and mixed under the placed source at render.py's send. On headphones the
/// source goes through an HRTF (AVAudioEnvironmentNode); on speakers it is panned with an equal-power law. Farther
/// from the notch it is a little quieter, duller and roomier. The engine runs only while sounds play (and briefly
/// after, so a run of cues stays warm) or while kept warm, and stops at once on `stopAll()`. Every AVFAudio call
/// that can raise an Objective-C exception goes through AudioCapture's wrappers.
@MainActor
final class FerbSoundEngine: FerbSoundPlaying {
    /// Today's master volume (SoundFeedback); at it, the family plays at the levels it was decided at.
    static let defaultVolume = 0.18
    static let idleSeconds = 10.0
    /// Every sound is scheduled this far ahead, so its source and its room start on the same sample.
    static let scheduleLead = 0.02
    private static let stereo = AVAudioFormat(standardFormatWithSampleRate: FerbSoundData.sampleRate, channels: 2)!
    private static let mono = AVAudioFormat(standardFormatWithSampleRate: FerbSoundData.sampleRate, channels: 1)!

    /// The device's latency (measured) plus the engine's own scheduling lead.
    var outputLatency: TimeInterval { measuredLatency + Self.scheduleLead }
    private var measuredLatency: TimeInterval = 0
    private let engine: AVAudioEngine
    private let environment = AVAudioEnvironmentNode()
    private var stereoPlayers: [AVAudioPlayerNode] = []
    private var spatialPlayers: [AVAudioPlayerNode] = []
    private var graphReady = false
    private var nextStereo = 0
    private var nextSpatial = 0
    private let library = FerbSoundLibrary()
    private let defaults: UserDefaults
    private let preference: @MainActor (String) -> Any?
    private let headphones: @MainActor () -> Bool
    private let screens: @MainActor () -> [(frame: CGRect, notch: CGPoint)]
    private let measureLatency: @MainActor () -> TimeInterval
    private let idleSleep: @Sendable (TimeInterval) async -> Void
    private var warm = false
    private var busyUntil = Date.distantPast
    private var idleStop: Task<Void, Never>?
    private var routeObserver: NSObjectProtocol?

    init(engine: AVAudioEngine = AVAudioEngine(), defaults: UserDefaults = .standard,
         preference: @escaping @MainActor (String) -> Any? = { AudioOutputFacts.globalPreference($0) },
         headphones: @escaping @MainActor () -> Bool = { AudioOutputFacts.headphonesAreTheOutput() },
         screens: @escaping @MainActor () -> [(frame: CGRect, notch: CGPoint)] = { FerbSoundEngine.screensWithNotches() },
         measureLatency: @escaping @MainActor () -> TimeInterval = { AudioOutputFacts.outputLatency() },
         idleSleep: @escaping @Sendable (TimeInterval) async -> Void = { try? await Task.sleep(for: .seconds($0)) }) {
        self.engine = engine
        self.defaults = defaults
        self.preference = preference
        self.headphones = headphones
        self.screens = screens
        self.measureLatency = measureLatency
        self.idleSleep = idleSleep
        measuredLatency = measureLatency()
        // The output changed (headphones, a display's speakers): AVAudioEngine has stopped itself. Players are reset
        // so the next sound starts cleanly; keeping warm survives the change.
        routeObserver = NotificationCenter.default.addObserver(forName: .AVAudioEngineConfigurationChange,
                                                               object: engine, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.outputChanged() }
        }
    }

    deinit {
        if let routeObserver { NotificationCenter.default.removeObserver(routeObserver) }
        idleStop?.cancel()
        FERBStopOutput(engine)
    }

    /// The gain at today's settings, before placement: FERB's volume (relative to the default) times the alert
    /// volume. Nil when sound is off (the system's UI sounds or FERB's feedback).
    var settingsGain: Float? {
        guard SoundPolicy.shouldPlay(.done, uiSoundsEnabled: AudioOutputFacts.uiSoundsEnabled(preference),
                                     feedbackEnabled: defaults.object(forKey: "feedback.enabled") as? Bool ?? true)
        else { return nil }
        let volume = min(max(defaults.object(forKey: "feedback.volume") as? Double ?? Self.defaultVolume, 0), 0.5)
        let gain = Float(volume / Self.defaultVolume) * AudioOutputFacts.alertVolume(preference)
        return gain > 0 ? gain : nil
    }

    var isRunning: Bool { engine.isRunning }

    func play(_ s: FerbSound, at point: CGPoint?) {
        guard SoundPolicy.shouldPlay(s, uiSoundsEnabled: AudioOutputFacts.uiSoundsEnabled(preference),
                                     feedbackEnabled: defaults.object(forKey: "feedback.enabled") as? Bool ?? true),
              let gain = settingsGain else { return }
        let subtle = defaults.string(forKey: "feedback.intensity") == "subtle"
        guard let url = FerbSoundFiles.files(for: s, subtle: subtle).randomElement(),
              let data = library.data(for: url, roomSend: FerbSoundFiles.isFinishedMix(s)
                                          ? nil : FerbSoundFiles.roomSend(s, subtle: subtle)),
              startIfNeeded() else { return }
        let start = AVAudioTime(hostTime: mach_absolute_time() + AVAudioTime.hostTime(forSeconds: Self.scheduleLead))
        var seconds = Double(data.channels[0].count) / FerbSoundData.sampleRate
        if data.channels.count == 2 {
            schedule(data.channels, on: nextPlayer(&stereoPlayers, &nextStereo), volume: gain, at: start)
        } else {
            let place = SoundPlacement.of(point, screens: screens())
            let level = gain * pow(10, place.gainDB / 20)
            let dry = place.distance > 0.02
                ? FerbSoundLibrary.lowPass(data.channels[0], cutoff: 12_000 * pow(5_000.0 / 12_000, Double(place.distance)))
                : data.channels[0]
            let roomier = Float(1 + 0.5 * place.distance)          // a little more room farther from the notch
            let room = data.room?.map { $0.map { $0 * roomier } }
            seconds = Double(max(dry.count, room?.first?.count ?? 0)) / FerbSoundData.sampleRate
            if headphones() {
                environment.outputType = .headphones
                let player = nextPlayer(&spatialPlayers, &nextSpatial)
                let azimuth = Double(place.pan) / 0.6 * Double.pi / 3          // the screen's edges at ±60°
                player.position = AVAudio3DPoint(x: Float(sin(azimuth)), y: 0, z: Float(-cos(azimuth)))
                schedule([dry], on: player, volume: level, at: start)
                if let room { schedule(room, on: nextPlayer(&stereoPlayers, &nextStereo), volume: level, at: start) }
            } else {
                // Equal power, scaled so the centre plays the source at full level on both sides as render.py mixes it.
                let angle = (place.pan + 1) * Float.pi / 4
                let sides = [Float(2).squareRoot() * cos(angle), Float(2).squareRoot() * sin(angle)]
                let mixed = sides.indices.map { c in
                    Self.sum(dry.map { $0 * sides[c] }, room?[c] ?? [])
                }
                schedule(mixed, on: nextPlayer(&stereoPlayers, &nextStereo), volume: level, at: start)
            }
        }
        busyUntil = max(busyUntil, Date().addingTimeInterval(seconds))
        scheduleIdleStop()
    }

    func stopAll() {
        warm = false
        halt()
    }

    func keepWarm(_ on: Bool) {
        warm = on
        if on {
            measuredLatency = measureLatency()     // a device switch while stopped may post no configuration change
            idleStop?.cancel()
            idleStop = nil
            if settingsGain != nil { _ = startIfNeeded() }    // pre-rolled, so the first cue isn't late
        } else if engine.isRunning {
            scheduleIdleStop()
        }
    }

    private func halt() {
        idleStop?.cancel()
        idleStop = nil
        busyUntil = .distantPast
        for player in stereoPlayers + spatialPlayers { FERBStopPlayer(player) }
        FERBStopOutput(engine)
    }

    private func outputChanged() {
        let keep = warm
        stopAll()
        measuredLatency = measureLatency()
        if keep { keepWarm(true) }
    }

    // MARK: - Engine

    private func startIfNeeded() -> Bool {
        if !graphReady {
            guard FERBAttachOutputNode(engine, environment, engine.mainMixerNode, Self.stereo) == nil else { return false }
            environment.reverbParameters.enable = false
            environment.distanceAttenuationParameters.rolloffFactor = 0  // distance is our own level and low-pass
            if spatialPlayers.isEmpty { spatialPlayers = (0..<3).map { _ in AVAudioPlayerNode() } }
            if stereoPlayers.isEmpty { stereoPlayers = (0..<4).map { _ in AVAudioPlayerNode() } }
            for player in spatialPlayers {
                guard FERBAttachOutputNode(engine, player, environment, Self.mono) == nil else { return false }
                player.renderingAlgorithm = .HRTFHQ
            }
            for player in stereoPlayers {
                guard FERBAttachOutputNode(engine, player, engine.mainMixerNode, Self.stereo) == nil else { return false }
            }
            graphReady = true
        }
        if engine.isRunning { return true }
        return FERBStartOutput(engine) == nil && engine.isRunning
    }

    private func nextPlayer(_ pool: inout [AVAudioPlayerNode], _ index: inout Int) -> AVAudioPlayerNode {
        defer { index = (index + 1) % pool.count }
        return pool[index]
    }

    private func schedule(_ channels: [[Float]], on player: AVAudioPlayerNode, volume: Float, at start: AVAudioTime) {
        let format = channels.count == 2 ? Self.stereo : Self.mono
        let frames = channels.map(\.count).max() ?? 0
        guard frames > 0, let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(frames)),
              let out = buffer.floatChannelData else { return }
        buffer.frameLength = AVAudioFrameCount(frames)
        for (c, samples) in channels.enumerated() {
            out[c].initialize(repeating: 0, count: frames)
            samples.withUnsafeBufferPointer { out[c].update(from: $0.baseAddress!, count: samples.count) }
        }
        player.volume = volume
        _ = FERBPlayBuffer(player, buffer, start)    // a reused player gives up its old sound
    }

    private func scheduleIdleStop() {
        idleStop?.cancel()
        idleStop = nil
        guard !warm else { return }
        let wait = max(0, busyUntil.timeIntervalSinceNow) + Self.idleSeconds
        let sleep = idleSleep
        idleStop = Task { [weak self] in
            await sleep(wait)
            guard !Task.isCancelled, let self, !self.warm else { return }
            self.halt()
        }
    }

    private static func sum(_ a: [Float], _ b: [Float]) -> [Float] {
        var out = [Float](repeating: 0, count: max(a.count, b.count))
        for i in a.indices { out[i] += a[i] }
        for i in b.indices { out[i] += b[i] }
        return out
    }

    static func screensWithNotches() -> [(frame: CGRect, notch: CGPoint)] {
        NSScreen.screens.map { ($0.frame, OrbController.notchCentre(of: $0)) }
    }
}
