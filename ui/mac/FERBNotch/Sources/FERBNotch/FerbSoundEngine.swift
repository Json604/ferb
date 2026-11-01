import AppKit
import AVFAudio
import AudioCapture
import CoreAudio
import NotchCore

/// What plays FERB's sounds; the intro's controller and its tests take this, not the engine.
@MainActor
protocol FerbSoundPlaying: AnyObject {
    /// `point` is in AppKit global coordinates; nil plays it centred at the notch.
    func play(_ s: FerbSound, at point: CGPoint?)
    func stopAll()
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
/// after, so a run of cues stays warm), and stops at once on `stopAll()`.
@MainActor
final class FerbSoundEngine: FerbSoundPlaying {
    /// Today's master volume (SoundFeedback); at it, the family plays at the levels it was decided at.
    static let defaultVolume = 0.18
    static let idleSeconds = 10.0
    private static let stereo = AVAudioFormat(standardFormatWithSampleRate: FerbSoundData.sampleRate, channels: 2)!
    private static let mono = AVAudioFormat(standardFormatWithSampleRate: FerbSoundData.sampleRate, channels: 1)!

    private let engine: AVAudioEngine
    private let environment = AVAudioEnvironmentNode()
    private var stereoPlayers: [AVAudioPlayerNode] = []
    private var spatialPlayers: [AVAudioPlayerNode] = []
    private var nextStereo = 0
    private var nextSpatial = 0
    private let library = FerbSoundLibrary()
    private let defaults: UserDefaults
    private let headphones: @MainActor () -> Bool
    private let screens: @MainActor () -> [(frame: CGRect, notch: CGPoint)]
    private var busyUntil = Date.distantPast
    private var idleStop: Task<Void, Never>?

    init(engine: AVAudioEngine = AVAudioEngine(), defaults: UserDefaults = .standard,
         headphones: @escaping @MainActor () -> Bool = { FerbSoundEngine.headphonesAreTheOutput() },
         screens: @escaping @MainActor () -> [(frame: CGRect, notch: CGPoint)] = { FerbSoundEngine.screensWithNotches() }) {
        self.engine = engine
        self.defaults = defaults
        self.headphones = headphones
        self.screens = screens
    }

    func play(_ s: FerbSound, at point: CGPoint?) {
        guard SoundPolicy.shouldPlay(s, uiSoundsEnabled: Self.systemUISoundsEnabled(),
                                     feedbackEnabled: defaults.object(forKey: "feedback.enabled") as? Bool ?? true)
        else { return }
        let volume = min(max(defaults.object(forKey: "feedback.volume") as? Double ?? Self.defaultVolume, 0), 0.5)
        let subtle = defaults.string(forKey: "feedback.intensity") == "subtle"
        guard volume > 0, let url = FerbSoundFiles.files(for: s, subtle: subtle).randomElement(),
              let data = library.data(for: url, roomSend: FerbSoundFiles.isFinishedMix(s)
                                          ? nil : FerbSoundFiles.roomSend(s, subtle: subtle)),
              startIfNeeded() else { return }
        let gain = Float(volume / Self.defaultVolume)
        let start = AVAudioTime(hostTime: mach_absolute_time() + AVAudioTime.hostTime(forSeconds: 0.02))
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
        idleStop?.cancel()
        idleStop = nil
        busyUntil = .distantPast
        for player in stereoPlayers + spatialPlayers { player.stop() }
        FERBStopMagneticOutput(engine)
    }

    // MARK: - Engine

    private func startIfNeeded() -> Bool {
        if stereoPlayers.isEmpty {
            engine.attach(environment)
            engine.connect(environment, to: engine.mainMixerNode, format: Self.stereo)
            environment.reverbParameters.enable = false
            environment.distanceAttenuationParameters.rolloffFactor = 0  // distance is our own level and low-pass
            for _ in 0..<3 {
                let player = AVAudioPlayerNode()
                engine.attach(player)
                engine.connect(player, to: environment, format: Self.mono)
                player.renderingAlgorithm = .HRTFHQ
                spatialPlayers.append(player)
            }
            for _ in 0..<4 {
                let player = AVAudioPlayerNode()
                engine.attach(player)
                engine.connect(player, to: engine.mainMixerNode, format: Self.stereo)
                stereoPlayers.append(player)
            }
        }
        if engine.isRunning { return true }
        return FERBStartMagneticOutput(engine) == nil && engine.isRunning
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
        player.stop()                        // a reused player gives up its old sound
        player.volume = volume
        player.scheduleBuffer(buffer, at: nil, options: [], completionHandler: nil)
        player.play(at: start)
    }

    private func scheduleIdleStop() {
        idleStop?.cancel()
        let wait = max(0, busyUntil.timeIntervalSinceNow) + Self.idleSeconds
        idleStop = Task { [weak self] in
            try? await Task.sleep(for: .seconds(wait))
            guard !Task.isCancelled, let self else { return }
            for player in self.stereoPlayers + self.spatialPlayers { player.stop() }
            FERBStopMagneticOutput(self.engine)
            self.idleStop = nil
        }
    }

    private static func sum(_ a: [Float], _ b: [Float]) -> [Float] {
        var out = [Float](repeating: 0, count: max(a.count, b.count))
        for i in a.indices { out[i] += a[i] }
        for i in b.indices { out[i] += b[i] }
        return out
    }

    // MARK: - System facts

    /// "Play user interface sound effects" (System Settings → Sound); absent means on.
    static func systemUISoundsEnabled() -> Bool {
        CFPreferencesAppSynchronize(kCFPreferencesAnyApplication)
        let value = CFPreferencesCopyAppValue("com.apple.sound.uiaudio.enabled" as CFString, kCFPreferencesAnyApplication)
        return (value as? NSNumber)?.boolValue ?? true
    }

    /// The default output is headphones: the built-in output's headphone jack, or a Bluetooth device (AirPods and
    /// other headsets, the common case; a Bluetooth speaker also takes this path).
    static func headphonesAreTheOutput() -> Bool {
        var device = AudioDeviceID(0)
        var size = UInt32(MemoryLayout<AudioDeviceID>.size)
        var address = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyDefaultOutputDevice,
                                                 mScope: kAudioObjectPropertyScopeGlobal,
                                                 mElement: kAudioObjectPropertyElementMain)
        guard AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &device) == noErr,
              device != 0 else { return false }
        var transport = UInt32(0)
        size = UInt32(MemoryLayout<UInt32>.size)
        address.mSelector = kAudioDevicePropertyTransportType
        guard AudioObjectGetPropertyData(device, &address, 0, nil, &size, &transport) == noErr else { return false }
        if transport == kAudioDeviceTransportTypeBluetooth || transport == kAudioDeviceTransportTypeBluetoothLE {
            return true
        }
        guard transport == kAudioDeviceTransportTypeBuiltIn else { return false }
        var source = UInt32(0)
        address.mSelector = kAudioDevicePropertyDataSource
        address.mScope = kAudioDevicePropertyScopeOutput
        guard AudioObjectGetPropertyData(device, &address, 0, nil, &size, &source) == noErr else { return false }
        return source == 0x6864_706E                                   // 'hdpn', the headphone jack
    }

    static func screensWithNotches() -> [(frame: CGRect, notch: CGPoint)] {
        NSScreen.screens.map { ($0.frame, OrbController.notchCentre(of: $0)) }
    }
}
