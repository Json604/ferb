// Spikes 9 + 17 (throwaway, not FERB code): system-audio taps, microphone + on-device speech, permission states.
// Usage (via `open -W FERBAudioSpike.app --args <mode> ...`), results appended as JSON lines to /tmp/ferb-audio-spike.jsonl:
//   status                      permission states
//   tap-system <seconds>        Core Audio tap of the whole output mix (FERB's own process excluded)
//   tap-pid <pid> <seconds>     Core Audio tap of one process
//   mic <seconds>               microphone capture + on-device transcription
import AppKit
import AVFoundation
import CoreAudio
import Speech

let OUT = "/tmp/ferb-audio-spike.jsonl"; let BUILD_TAG = "1790829257"

func emit(_ d: [String: Any]) {
    var d = d
    d["at"] = ISO8601DateFormatter().string(from: Date())
    let data = try! JSONSerialization.data(withJSONObject: d)
    let line = String(data: data, encoding: .utf8)! + "\n"
    if let h = FileHandle(forWritingAtPath: OUT) { h.seekToEndOfFile(); h.write(line.data(using: .utf8)!); h.closeFile() }
    else { FileManager.default.createFile(atPath: OUT, contents: line.data(using: .utf8)) }
}

func prop<T>(_ obj: AudioObjectID, _ sel: AudioObjectPropertySelector, _ initial: T) -> (OSStatus, T) {
    var addr = AudioObjectPropertyAddress(mSelector: sel, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
    var v = initial
    var size = UInt32(MemoryLayout<T>.size)
    let st = AudioObjectGetPropertyData(obj, &addr, 0, nil, &size, &v)
    return (st, v)
}

final class Meter { var frames = 0; var sumSq = 0.0; var peak: Float = 0 }

func tap(pid: pid_t?, seconds: Double) {
    let meter = Meter()
    let desc: CATapDescription
    if let pid = pid {
        var addr = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyTranslatePIDToProcessObject,
                                              mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var p = pid; var obj = AudioObjectID(kAudioObjectUnknown); var size = UInt32(MemoryLayout<AudioObjectID>.size)
        let st = AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &addr, UInt32(MemoryLayout<pid_t>.size), &p, &size, &obj)
        if st != noErr || obj == kAudioObjectUnknown { emit(["mode": "tap-pid", "error": "no audio object for pid \(pid) (\(st))"]); return }
        desc = CATapDescription(stereoMixdownOfProcesses: [obj])
    } else {
        var addr = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyTranslatePIDToProcessObject,
                                              mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var me = getpid(); var myObj = AudioObjectID(kAudioObjectUnknown); var size = UInt32(MemoryLayout<AudioObjectID>.size)
        _ = AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &addr, UInt32(MemoryLayout<pid_t>.size), &me, &size, &myObj)
        desc = CATapDescription(stereoGlobalTapButExcludeProcesses: myObj == kAudioObjectUnknown ? [] : [myObj])
    }
    desc.uuid = UUID()
    desc.muteBehavior = .unmuted
    desc.isPrivate = true
    var tapID = AudioObjectID(kAudioObjectUnknown)
    var st = AudioHardwareCreateProcessTap(desc, &tapID)
    if st != noErr { emit(["mode": "tap", "error": "AudioHardwareCreateProcessTap \(st)"]); return }
    let (_, outDev) = prop(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDefaultSystemOutputDevice, AudioObjectID(0))
    let (_, uidRef) = prop(outDev, kAudioDevicePropertyDeviceUID, "" as CFString)
    let (_, nameRef) = prop(outDev, kAudioObjectPropertyName, "" as CFString)
    let outUID = uidRef as String
    let agg: [String: Any] = [
        kAudioAggregateDeviceNameKey: "FERB-Spike-Tap", kAudioAggregateDeviceUIDKey: UUID().uuidString,
        kAudioAggregateDeviceMainSubDeviceKey: outUID, kAudioAggregateDeviceIsPrivateKey: true,
        kAudioAggregateDeviceIsStackedKey: false, kAudioAggregateDeviceTapAutoStartKey: true,
        kAudioAggregateDeviceSubDeviceListKey: [[kAudioSubDeviceUIDKey: outUID]],
        kAudioAggregateDeviceTapListKey: [[kAudioSubTapDriftCompensationKey: true, kAudioSubTapUIDKey: desc.uuid.uuidString]]]
    var aggID = AudioObjectID(kAudioObjectUnknown)
    st = AudioHardwareCreateAggregateDevice(agg as CFDictionary, &aggID)
    if st != noErr { emit(["mode": "tap", "error": "AudioHardwareCreateAggregateDevice \(st)"]); AudioHardwareDestroyProcessTap(tapID); return }
    var procID: AudioDeviceIOProcID?
    st = AudioDeviceCreateIOProcIDWithBlock(&procID, aggID, nil) { _, inData, _, _, _ in
        let abl = UnsafeMutableAudioBufferListPointer(UnsafeMutablePointer(mutating: inData))
        for b in abl {
            guard let p = b.mData else { continue }
            let n = Int(b.mDataByteSize) / MemoryLayout<Float>.size
            let f = p.bindMemory(to: Float.self, capacity: n)
            for i in 0..<n { let v = f[i]; meter.sumSq += Double(v * v); meter.peak = max(meter.peak, abs(v)) }
            meter.frames += n
        }
    }
    if st != noErr { emit(["mode": "tap", "error": "IOProc \(st)"]); return }
    let t0 = Date()
    st = AudioDeviceStart(aggID, procID)
    if st != noErr { emit(["mode": "tap", "error": "AudioDeviceStart \(st)"]) }
    Thread.sleep(forTimeInterval: seconds)
    AudioDeviceStop(aggID, procID)
    AudioDeviceDestroyIOProcID(aggID, procID!)
    AudioHardwareDestroyAggregateDevice(aggID)
    AudioHardwareDestroyProcessTap(tapID)
    let rms = meter.frames > 0 ? sqrt(meter.sumSq / Double(meter.frames)) : 0
    emit(["mode": pid == nil ? "tap-system" : "tap-pid", "pid": pid.map { Int($0) } ?? 0, "seconds": Date().timeIntervalSince(t0),
          "samples": meter.frames, "rms": rms, "peak": Double(meter.peak), "silent": meter.peak < 0.0005,
          "output_device": nameRef as String])
}

func status() {
    let mic: String = { switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized: return "granted"; case .denied: return "denied"; case .restricted: return "restricted"
        default: return "not_determined" } }()
    let sr: String = { switch SFSpeechRecognizer.authorizationStatus() {
        case .authorized: return "granted"; case .denied: return "denied"; case .restricted: return "restricted"
        default: return "not_determined" } }()
    let rec = SFSpeechRecognizer(locale: Locale(identifier: "en-US"))
    emit(["build": BUILD_TAG, "mode": "status", "microphone": mic, "speech": sr, "on_device_speech": rec?.supportsOnDeviceRecognition ?? false])
}

func builtInMic() -> AudioObjectID? {
    var addr = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyDevices, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
    var size: UInt32 = 0
    AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &addr, 0, nil, &size)
    var ids = [AudioObjectID](repeating: 0, count: Int(size) / MemoryLayout<AudioObjectID>.size)
    AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &addr, 0, nil, &size, &ids)
    for id in ids {
        let (_, transport) = prop(id, kAudioDevicePropertyTransportType, UInt32(0))
        var sa = AudioObjectPropertyAddress(mSelector: kAudioDevicePropertyStreams, mScope: kAudioObjectPropertyScopeInput, mElement: kAudioObjectPropertyElementMain)
        var ss: UInt32 = 0
        AudioObjectGetPropertyDataSize(id, &sa, 0, nil, &ss)
        if transport == kAudioDeviceTransportTypeBuiltIn && ss > 0 { return id }
    }
    return nil
}

func level(seconds: Double, builtin: Bool = false) {           // input level only: no transcription, nothing kept
    let engine = AVAudioEngine()
    let input = engine.inputNode
    var inDev = prop(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDefaultInputDevice, AudioObjectID(0)).1
    if builtin, let b = builtInMic(), let unit = input.audioUnit {   // this app only: the system default is unchanged
        var dev = b
        AudioUnitSetProperty(unit, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0, &dev, UInt32(MemoryLayout<AudioObjectID>.size))
        inDev = b
    }
    let (_, nameRef) = prop(inDev, kAudioObjectPropertyName, "" as CFString)
    let fmt = input.outputFormat(forBus: 0)
    let meter = Meter()
    input.installTap(onBus: 0, bufferSize: 1024, format: fmt) { buf, _ in
        if let ch = buf.floatChannelData?[0] { for i in 0..<Int(buf.frameLength) { let v = ch[i]; meter.sumSq += Double(v*v); meter.peak = max(meter.peak, abs(v)) }; meter.frames += Int(buf.frameLength) }
    }
    do { try engine.start() } catch { emit(["mode": "level", "error": "engine: \(error)"]); return }
    Thread.sleep(forTimeInterval: seconds)
    engine.stop()
    emit(["mode": "level", "input_device": nameRef as String, "sample_rate": fmt.sampleRate, "channels": Int(fmt.channelCount),
          "frames": meter.frames, "rms": meter.frames > 0 ? sqrt(meter.sumSq / Double(meter.frames)) : 0, "peak": Double(meter.peak)])
}

func ioLevel(seconds: Double, builtin: Bool) {   // the input device's own IOProc (like the system-audio tap)
    var dev = prop(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDefaultInputDevice, AudioObjectID(0)).1
    if builtin, let b = builtInMic() { dev = b }
    let (_, nameRef) = prop(dev, kAudioObjectPropertyName, "" as CFString)
    let meter = Meter()
    var procID: AudioDeviceIOProcID?
    var st = AudioDeviceCreateIOProcIDWithBlock(&procID, dev, nil) { _, inData, _, _, _ in
        let abl = UnsafeMutableAudioBufferListPointer(UnsafeMutablePointer(mutating: inData))
        for b in abl {
            guard let p = b.mData else { continue }
            let n = Int(b.mDataByteSize) / MemoryLayout<Float>.size
            let f = p.bindMemory(to: Float.self, capacity: n)
            for i in 0..<n { let v = f[i]; meter.sumSq += Double(v * v); meter.peak = max(meter.peak, abs(v)) }
            meter.frames += n
        }
    }
    if st == noErr { st = AudioDeviceStart(dev, procID) }
    Thread.sleep(forTimeInterval: seconds)
    AudioDeviceStop(dev, procID)
    if let p = procID { AudioDeviceDestroyIOProcID(dev, p) }
    emit(["mode": "io-level", "status": Int(st), "input_device": nameRef as String, "frames": meter.frames,
          "rms": meter.frames > 0 ? sqrt(meter.sumSq / Double(meter.frames)) : 0, "peak": Double(meter.peak)])
}

func transcribeFile(_ path: String) {     // on-device speech on a recorded sentence: accuracy and latency
    guard let rec = SFSpeechRecognizer(locale: Locale(identifier: "en-US")) else { emit(["mode": "file", "error": "no recognizer"]); return }
    let req = SFSpeechURLRecognitionRequest(url: URL(fileURLWithPath: path))
    req.requiresOnDeviceRecognition = rec.supportsOnDeviceRecognition
    let sem = DispatchSemaphore(value: 0)
    let t0 = Date()
    let task = rec.recognitionTask(with: req) { r, err in
        if let r = r, r.isFinal { emit(["mode": "file", "text": r.bestTranscription.formattedString, "on_device": req.requiresOnDeviceRecognition, "seconds": Date().timeIntervalSince(t0)]); sem.signal() }
        else if let err = err { emit(["mode": "file", "error": err.localizedDescription]); sem.signal() }
    }
    _ = sem.wait(timeout: .now() + 60); _ = task
}

func mic2(seconds: Double, builtin: Bool) {   // the live path FERB would use: IOProc -> on-device speech
    guard let rec = SFSpeechRecognizer(locale: Locale(identifier: "en-US")) else { return }
    var dev = prop(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDefaultInputDevice, AudioObjectID(0)).1
    if builtin, let b = builtInMic() { dev = b }
    let (_, nameRef) = prop(dev, kAudioObjectPropertyName, "" as CFString)
    var fa = AudioObjectPropertyAddress(mSelector: kAudioDevicePropertyStreamFormat, mScope: kAudioObjectPropertyScopeInput, mElement: kAudioObjectPropertyElementMain)
    var asbd = AudioStreamBasicDescription(); var sz = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
    AudioObjectGetPropertyData(dev, &fa, 0, nil, &sz, &asbd)
    guard let fmt = AVAudioFormat(standardFormatWithSampleRate: asbd.mSampleRate, channels: 1) else { return }
    let req = SFSpeechAudioBufferRecognitionRequest()
    req.requiresOnDeviceRecognition = rec.supportsOnDeviceRecognition
    let chans = Int(max(1, asbd.mChannelsPerFrame))
    let meter = Meter()
    var procID: AudioDeviceIOProcID?
    AudioDeviceCreateIOProcIDWithBlock(&procID, dev, nil) { _, inData, _, _, _ in
        let abl = UnsafeMutableAudioBufferListPointer(UnsafeMutablePointer(mutating: inData))
        guard let b = abl.first, let p = b.mData else { return }
        let n = Int(b.mDataByteSize) / MemoryLayout<Float>.size / chans
        guard n > 0, let pcm = AVAudioPCMBuffer(pcmFormat: fmt, frameCapacity: AVAudioFrameCount(n)) else { return }
        pcm.frameLength = AVAudioFrameCount(n)
        let src = p.bindMemory(to: Float.self, capacity: n * chans)
        for i in 0..<n { let v = src[i * chans]; pcm.floatChannelData![0][i] = v; meter.sumSq += Double(v*v); meter.peak = max(meter.peak, abs(v)) }
        meter.frames += n
        req.append(pcm)
    }
    let sem = DispatchSemaphore(value: 0)
    var released = Date()
    let task = rec.recognitionTask(with: req) { r, err in
        if let r = r, r.isFinal { emit(["mode": "mic2", "text": r.bestTranscription.formattedString, "input_device": nameRef as String,
                                       "after_release_s": Date().timeIntervalSince(released), "peak": Double(meter.peak)]); sem.signal() }
        else if let err = err { emit(["mode": "mic2", "error": err.localizedDescription, "input_device": nameRef as String, "peak": Double(meter.peak)]); sem.signal() }
    }
    AudioDeviceStart(dev, procID)
    Thread.sleep(forTimeInterval: seconds)
    AudioDeviceStop(dev, procID)
    released = Date()
    req.endAudio()
    _ = sem.wait(timeout: .now() + 30); _ = task
    if let p = procID { AudioDeviceDestroyIOProcID(dev, p) }
}

func request() {                       // ask for both permissions, record nothing
    let sem = DispatchSemaphore(value: 0)
    AVCaptureDevice.requestAccess(for: .audio) { mic in
        SFSpeechRecognizer.requestAuthorization { st in
            emit(["mode": "request", "microphone": mic, "speech": st == .authorized]); sem.signal() } }
    _ = sem.wait(timeout: .now() + 120)
}

func mic(seconds: Double) {
    let sem = DispatchSemaphore(value: 0)
    AVCaptureDevice.requestAccess(for: .audio) { ok in
        if !ok { emit(["mode": "mic", "error": "microphone denied"]); sem.signal(); return }
        SFSpeechRecognizer.requestAuthorization { st in
            guard st == .authorized, let rec = SFSpeechRecognizer(locale: Locale(identifier: "en-US")) else {
                emit(["mode": "mic", "error": "speech recognition not authorized (\(st.rawValue))"]); sem.signal(); return }
            let engine = AVAudioEngine()
            let req = SFSpeechAudioBufferRecognitionRequest()
            req.requiresOnDeviceRecognition = rec.supportsOnDeviceRecognition
            req.shouldReportPartialResults = false
            let input = engine.inputNode
            let fmt = input.outputFormat(forBus: 0)
            let meter = Meter()
            input.installTap(onBus: 0, bufferSize: 1024, format: fmt) { buf, _ in
                req.append(buf)
                if let ch = buf.floatChannelData?[0] { for i in 0..<Int(buf.frameLength) { let v = ch[i]; meter.sumSq += Double(v*v); meter.peak = max(meter.peak, abs(v)) }; meter.frames += Int(buf.frameLength) }
            }
            do { try engine.start() } catch { emit(["mode": "mic", "error": "engine: \(error)"]); sem.signal(); return }
            let t0 = Date()
            var finalText = ""
            let task = rec.recognitionTask(with: req) { r, err in
                if let r = r, r.isFinal { finalText = r.bestTranscription.formattedString;
                    emit(["mode": "mic", "text": finalText, "on_device": req.requiresOnDeviceRecognition,
                          "recorded_s": seconds, "transcribed_after_release_s": Date().timeIntervalSince(t0) - seconds,
                          "rms": meter.frames > 0 ? sqrt(meter.sumSq / Double(meter.frames)) : 0, "peak": Double(meter.peak)]); sem.signal() }
                else if let err = err { emit(["mode": "mic", "error": "recognition: \(err.localizedDescription)", "text": finalText]); sem.signal() }
            }
            DispatchQueue.global().asyncAfter(deadline: .now() + seconds) { engine.stop(); input.removeTap(onBus: 0); req.endAudio() }
            _ = task
        }
    }
    _ = sem.wait(timeout: .now() + seconds + 30)
}

let args = Array(CommandLine.arguments.dropFirst())
NSApplication.shared.setActivationPolicy(.accessory)
DispatchQueue.global().async {
    switch args.first ?? "status" {
    case "tap-system": tap(pid: nil, seconds: Double(args.dropFirst().first ?? "5") ?? 5)
    case "tap-pid": tap(pid: pid_t(args[1]) ?? 0, seconds: Double(args.count > 2 ? args[2] : "5") ?? 5)
    case "mic": mic(seconds: Double(args.dropFirst().first ?? "5") ?? 5)
    case "request": request()
    case "file": transcribeFile(args[1])
    case "mic2": mic2(seconds: Double(args.count > 1 ? args[1] : "6") ?? 6, builtin: args.contains("builtin"))
    case "io-level": ioLevel(seconds: Double(args.count > 1 ? args[1] : "3") ?? 3, builtin: args.contains("builtin"))
    case "level": level(seconds: Double(args.count > 1 ? args[1] : "3") ?? 3, builtin: args.contains("builtin"))
    default: status()
    }
    DispatchQueue.main.async { NSApp.terminate(nil) }
}
NSApplication.shared.run()
// rebuild 1790829244
