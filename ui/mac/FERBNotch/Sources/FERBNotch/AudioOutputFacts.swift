import AppKit
import CoreAudio
import Darwin
import NotchCore

/// What the system says about sound: the user's sound settings, the current output device, and which processes are
/// running microphone input.
enum AudioOutputFacts {
    /// A value from the global (any application) preferences, read fresh.
    static func globalPreference(_ key: String) -> Any? {
        CFPreferencesAppSynchronize(kCFPreferencesAnyApplication)
        return CFPreferencesCopyAppValue(key as CFString, kCFPreferencesAnyApplication)
    }

    /// "Play user interface sound effects" (System Settings → Sound); absent means on.
    static func uiSoundsEnabled(_ preference: (String) -> Any?) -> Bool {
        (preference("com.apple.sound.uiaudio.enabled") as? NSNumber)?.boolValue ?? true
    }

    /// The alert volume (System Settings → Sound), 0 to 1; absent means full.
    static func alertVolume(_ preference: (String) -> Any?) -> Float {
        min(max((preference("com.apple.sound.beep.volume") as? NSNumber)?.floatValue ?? 1, 0), 1)
    }

    /// The default output is headphones: the built-in output's headphone jack, or a Bluetooth device (AirPods and
    /// other headsets, the common case; a Bluetooth speaker also takes this path).
    static func headphonesAreTheOutput() -> Bool {
        guard let device = defaultOutputDevice(),
              let transport: UInt32 = property(device, kAudioDevicePropertyTransportType) else { return false }
        if transport == kAudioDeviceTransportTypeBluetooth || transport == kAudioDeviceTransportTypeBluetoothLE {
            return true
        }
        guard transport == kAudioDeviceTransportTypeBuiltIn,
              let source: UInt32 = property(device, kAudioDevicePropertyDataSource, scope: kAudioDevicePropertyScopeOutput)
        else { return false }
        return source == 0x6864_706E                                   // 'hdpn', the headphone jack
    }

    /// How long a sound takes from being scheduled to leaving the current output device: the device's, its first
    /// output stream's and the safety offset's latencies plus one I/O buffer, in seconds (Core Audio). 0 if unknown.
    static func outputLatency() -> TimeInterval {
        guard let device = defaultOutputDevice(),
              let rate: Float64 = property(device, kAudioDevicePropertyNominalSampleRate), rate > 0 else { return 0 }
        let output = kAudioDevicePropertyScopeOutput
        let deviceFrames: UInt32 = property(device, kAudioDevicePropertyLatency, scope: output) ?? 0
        let safety: UInt32 = property(device, kAudioDevicePropertySafetyOffset, scope: output) ?? 0
        let buffer: UInt32 = property(device, kAudioDevicePropertyBufferFrameSize, scope: output) ?? 0
        var streamFrames: UInt32 = 0
        var address = AudioObjectPropertyAddress(mSelector: kAudioDevicePropertyStreams, mScope: output,
                                                 mElement: kAudioObjectPropertyElementMain)
        var size = UInt32(0)
        if AudioObjectGetPropertyDataSize(device, &address, 0, nil, &size) == noErr, size >= 4 {
            var streams = [AudioStreamID](repeating: 0, count: Int(size) / MemoryLayout<AudioStreamID>.size)
            if AudioObjectGetPropertyData(device, &address, 0, nil, &size, &streams) == noErr, let first = streams.first {
                streamFrames = property(first, kAudioStreamPropertyLatency) ?? 0
            }
        }
        return latency(frames: [deviceFrames, streamFrames, safety, buffer], sampleRate: rate)
    }

    /// Frames to seconds, summed in floating point so a device's garbage value can't overflow; a total over a
    /// second is not a real output latency and counts as unknown (0).
    static func latency(frames: [UInt32], sampleRate: Double) -> TimeInterval {
        guard sampleRate > 0 else { return 0 }
        let seconds = frames.reduce(0.0) { $0 + Double($1) } / sampleRate
        return seconds.isFinite && seconds <= 1 ? seconds : 0
    }

    /// The processes running microphone input now: Core Audio's process objects (macOS 14) with an active input stream,
    /// by pid. A few property reads, made when a sound is about to play; empty when Core Audio doesn't say.
    static func processesRunningInput() -> [pid_t] {
        let system = AudioObjectID(kAudioObjectSystemObject)
        var address = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyProcessObjectList,
                                                 mScope: kAudioObjectPropertyScopeGlobal,
                                                 mElement: kAudioObjectPropertyElementMain)
        var size = UInt32(0)
        guard AudioObjectGetPropertyDataSize(system, &address, 0, nil, &size) == noErr, size > 0 else { return [] }
        var objects = [AudioObjectID](repeating: 0, count: Int(size) / MemoryLayout<AudioObjectID>.size)
        guard AudioObjectGetPropertyData(system, &address, 0, nil, &size, &objects) == noErr else { return [] }
        return objects.prefix(Int(size) / MemoryLayout<AudioObjectID>.size).compactMap { process in
            guard let running: UInt32 = property(process, kAudioProcessPropertyIsRunningInput), running != 0,
                  let pid: pid_t = property(process, kAudioProcessPropertyPID) else { return nil }
            return pid
        }
    }

    /// FERB's own processes: this app (its dictation) and anything run from inside its bundle (its helpers).
    static func isFerbProcess(_ pid: pid_t) -> Bool {
        if pid == getpid() { return true }
        var path = [CChar](repeating: 0, count: 4 * Int(MAXPATHLEN))
        guard proc_pidpath(pid, &path, UInt32(path.count)) > 0 else { return false }
        let bundle = Bundle.main.bundleURL.resolvingSymlinksInPath().path + "/"
        return URL(fileURLWithPath: String(cString: path)).resolvingSymlinksInPath().path.hasPrefix(bundle)
    }

    /// What MicrophoneUse needs to know about one process: whether it is an app (a running application that isn't
    /// background-only), the outermost app bundle holding its executable, the process macOS holds responsible for it
    /// and its parent. Nil when the process can't be read (gone, or another user's).
    static func processFacts(_ pid: pid_t) -> ProcessFacts? {
        var path = [CChar](repeating: 0, count: 4 * Int(MAXPATHLEN))
        guard proc_pidpath(pid, &path, UInt32(path.count)) > 0 else { return nil }
        var facts = ProcessFacts()
        if let app = NSRunningApplication(processIdentifier: pid), app.activationPolicy != .prohibited {
            facts.appName = app.localizedName ?? app.bundleIdentifier ?? "an app"
        }
        let components = URL(fileURLWithPath: String(cString: path)).pathComponents
        if let outer = components.firstIndex(where: { $0.hasSuffix(".app") }) {
            facts.bundleApp = String(components[outer].dropLast(4))
        }
        if let responsible = responsiblePID(pid), responsible > 0 { facts.responsible = responsible }
        var info = proc_bsdinfo()
        if proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, Int32(MemoryLayout<proc_bsdinfo>.size)) > 0 {
            facts.parent = pid_t(info.pbi_ppid)
        }
        return facts
    }

    /// The process macOS holds responsible for `pid` (an XPC service's app, as Activity Monitor shows it), from
    /// libsystem's responsibility call, resolved defensively like the other private symbols FERB uses (APP-563): nil
    /// when this macOS doesn't have it, and the parent chain is used alone.
    private static func responsiblePID(_ pid: pid_t) -> pid_t? {
        typealias Responsible = @convention(c) (pid_t) -> pid_t
        guard let symbol = responsibilitySymbol else { return nil }
        return unsafeBitCast(symbol, to: Responsible.self)(pid)
    }

    private static let responsibilitySymbol: UnsafeMutableRawPointer? = {
        dlsym(UnsafeMutableRawPointer(bitPattern: -2), "responsibility_get_pid_responsible_for_pid")   // RTLD_DEFAULT
    }()

    private static func defaultOutputDevice() -> AudioDeviceID? {
        let device: AudioDeviceID? = property(AudioObjectID(kAudioObjectSystemObject),
                                              kAudioHardwarePropertyDefaultOutputDevice)
        return device == 0 ? nil : device
    }

    private static func property<T>(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector,
                                    scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) -> T? {
        var address = AudioObjectPropertyAddress(mSelector: selector, mScope: scope,
                                                 mElement: kAudioObjectPropertyElementMain)
        var size = UInt32(MemoryLayout<T>.size)
        let value = UnsafeMutableRawPointer.allocate(byteCount: MemoryLayout<T>.size, alignment: MemoryLayout<T>.alignment)
        defer { value.deallocate() }
        guard AudioObjectGetPropertyData(object, &address, 0, nil, &size, value) == noErr else { return nil }
        return value.load(as: T.self)
    }
}
