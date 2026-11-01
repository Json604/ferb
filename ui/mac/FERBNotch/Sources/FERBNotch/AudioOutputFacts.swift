import AppKit
import CoreAudio

/// What the system says about sound output: the user's sound settings and the current output device.
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
        return Double(deviceFrames + streamFrames + safety + buffer) / rate
    }

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
