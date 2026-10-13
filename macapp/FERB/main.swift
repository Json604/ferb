// FERB.app (ARCHITECTURE section 20.1): push-to-talk's microphone, and FERB's on-device speech recognition. No
// windows, Dock icon or menu-bar item. It is the only part of FERB that opens a microphone. The daemon decides which
// input (never a Bluetooth headset's) and when: "start" opens it with a Core Audio IOProc and feeds on-device speech
// recognition; "stop" returns the text; "discard" drops everything. Microphone audio is never written anywhere.
// Speech is recognized on this Mac only (requiresOnDeviceRecognition): without on-device support FERB says so and
// never sends speech to Apple's servers (rule 15, L26).
// Usage: FERB                               run as the voice service (opened by the daemon)
//        FERB request                       ask for Microphone + Speech Recognition, print the result (ferb setup --voice)
//        FERB request-speech                ask for Speech Recognition only, print the result (ferb setup)
//        FERB transcribe <audio> <out> [locale]  L26: a captured app's sound file to text, written to <out> (0600)
//        FERB status <out>                  Speech Recognition state and on-device support, written to <out> (0600)
// The daemon starts the last two per use with `open -g -n -W` (LaunchServices: FERB.app, not the daemon, is the app
// macOS asks about); they never prompt and write nothing to stdout or any log.
import AppKit
import AVFoundation
import CoreAudio
import Speech

// ---------- the daemon's socket (newline-delimited JSON-RPC) ----------

final class Conn {
    let fd: Int32
    private var buffer = Data()
    init?(path: String) {
        fd = socket(AF_UNIX, SOCK_STREAM, 0)
        if fd < 0 { return nil }
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        guard bytes.count < MemoryLayout.size(ofValue: addr.sun_path) else { close(fd); return nil }
        withUnsafeMutableBytes(of: &addr.sun_path) { raw in for (i, b) in bytes.enumerated() { raw[i] = b } }
        let ok = withUnsafePointer(to: &addr) { p in
            p.withMemoryRebound(to: sockaddr.self, capacity: 1) { connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) } }
        if ok != 0 { close(fd); return nil }
    }
    deinit { close(fd) }
    func send(_ msg: [String: Any]) -> Bool {
        guard var data = try? JSONSerialization.data(withJSONObject: msg) else { return false }
        data.append(0x0a)
        return data.withUnsafeBytes { write(fd, $0.baseAddress, data.count) } == data.count
    }
    func readLine() -> [String: Any]? {
        while true {
            if let i = buffer.firstIndex(of: 0x0a) {
                let line = buffer.subdata(in: buffer.startIndex..<i)
                buffer.removeSubrange(buffer.startIndex...i)
                return (try? JSONSerialization.jsonObject(with: line)) as? [String: Any] ?? [:]
            }
            var chunk = [UInt8](repeating: 0, count: 65536)
            let n = read(fd, &chunk, chunk.count)
            if n <= 0 { return nil }
            buffer.append(contentsOf: chunk[0..<n])
        }
    }
    func call(_ id: Int, _ method: String, _ params: [String: Any]) -> [String: Any]? {
        guard send(["jsonrpc": "2.0", "id": id, "method": method, "params": params]) else { return nil }
        return readLine()
    }
}

func socketPath() -> String {
    let home = ProcessInfo.processInfo.environment["FERB_HOME"] ?? (NSHomeDirectory() + "/.ferb")
    return home + "/run/ferb.sock"
}

func connectAsVoice() -> Conn? {
    guard let c = Conn(path: socketPath()), let r = c.call(1, "auth.hello", ["role": "voice"]), r["result"] != nil else { return nil }
    return c
}

// ---------- audio devices (properties only: nothing is opened here) ----------

func getProp<T>(_ obj: AudioObjectID, _ sel: AudioObjectPropertySelector, _ scope: AudioObjectPropertyScope, _ initial: T) -> T {
    var addr = AudioObjectPropertyAddress(mSelector: sel, mScope: scope, mElement: kAudioObjectPropertyElementMain)
    var v = initial
    var size = UInt32(MemoryLayout<T>.size)
    _ = withUnsafeMutablePointer(to: &v) { AudioObjectGetPropertyData(obj, &addr, 0, nil, &size, $0) }
    return v
}

func fourCC(_ v: UInt32) -> String {
    String(bytes: [UInt8((v >> 24) & 0xff), UInt8((v >> 16) & 0xff), UInt8((v >> 8) & 0xff), UInt8(v & 0xff)], encoding: .ascii) ?? "????"
}

struct InputDevice { let id: AudioObjectID; let uid: String; let name: String; let transport: String; let isDefault: Bool }

func inputDevices() -> [InputDevice] {
    var addr = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyDevices, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
    var size: UInt32 = 0
    AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &addr, 0, nil, &size)
    var ids = [AudioObjectID](repeating: 0, count: Int(size) / MemoryLayout<AudioObjectID>.size)
    AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &addr, 0, nil, &size, &ids)
    let def = getProp(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDefaultInputDevice, kAudioObjectPropertyScopeGlobal, AudioObjectID(0))
    var out: [InputDevice] = []
    for id in ids {
        var sa = AudioObjectPropertyAddress(mSelector: kAudioDevicePropertyStreams, mScope: kAudioObjectPropertyScopeInput, mElement: kAudioObjectPropertyElementMain)
        var ss: UInt32 = 0
        AudioObjectGetPropertyDataSize(id, &sa, 0, nil, &ss)
        if ss == 0 { continue }
        let uid = getProp(id, kAudioDevicePropertyDeviceUID, kAudioObjectPropertyScopeGlobal, "" as CFString) as String
        let name = getProp(id, kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal, "" as CFString) as String
        let transport = fourCC(getProp(id, kAudioDevicePropertyTransportType, kAudioObjectPropertyScopeGlobal, UInt32(0)))
        out.append(InputDevice(id: id, uid: uid, name: name, transport: transport, isDefault: id == def))
    }
    return out
}

let BLUETOOTH: Set<String> = ["blue", "blea"]
let ON_DEVICE_UNAVAILABLE = "on-device speech recognition isn't available on this Mac"

// ---------- listening ----------

final class Listener {
    let recognizer = SFSpeechRecognizer(locale: Locale(identifier: "en-US"))
    private var device: AudioObjectID = 0
    private var procID: AudioDeviceIOProcID?
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var task: SFSpeechRecognitionTask?
    private var lastPartial = ""
    private var finalText: String?
    private let appendQ = DispatchQueue(label: "ferb.append")
    private var done = DispatchSemaphore(value: 0)

    func warmUp() {                                   // load the on-device model before anyone talks
        guard let rec = recognizer, rec.supportsOnDeviceRecognition else { return }
        let req = SFSpeechAudioBufferRecognitionRequest()
        req.requiresOnDeviceRecognition = true
        let sem = DispatchSemaphore(value: 0)
        let t = rec.recognitionTask(with: req) { r, err in if (r?.isFinal ?? false) || err != nil { sem.signal() } }
        if let f = AVAudioFormat(standardFormatWithSampleRate: 16000, channels: 1), let b = AVAudioPCMBuffer(pcmFormat: f, frameCapacity: 4000) {
            b.frameLength = 4000; req.append(b) }
        req.endAudio()
        _ = sem.wait(timeout: .now() + 10); _ = t
    }

    func start(uid: String) -> String? {
        guard let rec = recognizer else { return "speech recognition unavailable" }
        guard rec.supportsOnDeviceRecognition else { return ON_DEVICE_UNAVAILABLE }   // never the network (rule 15)
        guard let dev = inputDevices().first(where: { $0.uid == uid }) else { return "no such microphone" }
        if BLUETOOTH.contains(dev.transport) { return "refused: a Bluetooth microphone is never opened" }   // second guard
        var fa = AudioObjectPropertyAddress(mSelector: kAudioDevicePropertyStreamFormat, mScope: kAudioObjectPropertyScopeInput, mElement: kAudioObjectPropertyElementMain)
        var asbd = AudioStreamBasicDescription(); var sz = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
        AudioObjectGetPropertyData(dev.id, &fa, 0, nil, &sz, &asbd)
        guard let fmt = AVAudioFormat(standardFormatWithSampleRate: asbd.mSampleRate, channels: 1) else { return "bad format" }
        let req = SFSpeechAudioBufferRecognitionRequest()
        req.requiresOnDeviceRecognition = true
        req.shouldReportPartialResults = true
        req.contextualStrings = ["FERB"]
        lastPartial = ""; finalText = nil
        let sem = DispatchSemaphore(value: 0)               // this session's own: a discarded one can't end the next
        done = sem
        task = rec.recognitionTask(with: req) { [weak self] r, err in
            guard let self = self else { return }
            if let r = r {
                if r.isFinal { self.finalText = r.bestTranscription.formattedString; sem.signal() }
                else { self.lastPartial = r.bestTranscription.formattedString }
            } else if err != nil { sem.signal() }
        }
        let chans = Int(max(1, asbd.mChannelsPerFrame))
        let q = appendQ
        var pid: AudioDeviceIOProcID?
        let st = AudioDeviceCreateIOProcIDWithBlock(&pid, dev.id, nil) { _, inData, _, _, _ in
            let abl = UnsafeMutableAudioBufferListPointer(UnsafeMutablePointer(mutating: inData))
            guard let b = abl.first, let p = b.mData else { return }
            let n = Int(b.mDataByteSize) / MemoryLayout<Float>.size / chans
            guard n > 0, let pcm = AVAudioPCMBuffer(pcmFormat: fmt, frameCapacity: AVAudioFrameCount(n)) else { return }
            pcm.frameLength = AVAudioFrameCount(n)
            let src = p.bindMemory(to: Float.self, capacity: n * chans)
            for i in 0..<n { pcm.floatChannelData![0][i] = src[i * chans] }
            q.async { req.append(pcm) }
        }
        if st != noErr { return "couldn't open the microphone (\(st))" }
        device = dev.id; procID = pid; request = req
        AudioDeviceStart(dev.id, pid)                   // silent: the daemon sends "chime" once it's a real hold
        return nil
    }

    private func close() {
        if let p = procID { AudioDeviceStop(device, p); AudioDeviceDestroyIOProcID(device, p) }
        procID = nil
    }

    func stop(sound: Bool) -> String {
        close()
        appendQ.sync { request?.endAudio() }
        _ = done.wait(timeout: .now() + 3)
        let f = finalText ?? ""
        let text = f.isEmpty ? lastPartial : f            // an empty final keeps what was heard
        task = nil; request = nil
        if sound { NSSound(named: "Pop")?.play() }
        return text
    }

    func discard() {
        close()
        task?.cancel(); task = nil; request = nil; lastPartial = ""; finalText = nil
    }
}

// ---------- permissions ----------

func permissionStates() -> [String: String] {
    let mic: String = { switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized: return "granted"; case .denied: return "denied"; case .restricted: return "restricted"
        default: return "not_determined" } }()
    let sr: String = { switch SFSpeechRecognizer.authorizationStatus() {
        case .authorized: return "granted"; case .denied: return "denied"; case .restricted: return "restricted"
        default: return "not_determined" } }()
    return ["microphone": mic, "speech": sr]
}

func speechState() -> String {
    switch SFSpeechRecognizer.authorizationStatus() {
    case .authorized: return "granted"; case .denied: return "denied"; case .restricted: return "restricted"
    default: return "not_determined" }
}

func requestSpeech() {                               // ferb setup: Speech Recognition only, never the microphone
    let sem = DispatchSemaphore(value: 0)
    SFSpeechRecognizer.requestAuthorization { _ in sem.signal() }
    sem.wait()
    print("{\"speech\": \"\(speechState())\"}")
}

func requestPermissions() {
    let sem = DispatchSemaphore(value: 0)
    AVCaptureDevice.requestAccess(for: .audio) { _ in SFSpeechRecognizer.requestAuthorization { _ in sem.signal() } }
    _ = sem.wait(timeout: .now() + 300)
    let s = permissionStates()
    print("{\"microphone\": \"\(s["microphone"]!)\", \"speech\": \"\(s["speech"]!)\"}")
}

// ---------- hearing (L26): a captured app's sound to text, on this Mac only ----------

// The answer goes to the daemon's private folder: written to a 0600 file beside <out>, then renamed into place whole.
func writeAnswer(_ out: String, _ answer: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: answer) else { return }
    let part = out + ".part"
    let fd = open(part, O_WRONLY | O_CREAT | O_EXCL, 0o600)
    if fd < 0 { return }
    let wrote = data.withUnsafeBytes { write(fd, $0.baseAddress, data.count) }
    close(fd)
    if wrote == data.count { rename(part, out) } else { unlink(part) }
}

func recognizer(_ language: String?) -> SFSpeechRecognizer? {
    if let language = language { return SFSpeechRecognizer(locale: Locale(identifier: language)) }
    return SFSpeechRecognizer()                       // the Mac's own language
}

func writeStatus(_ out: String) {
    let rec = recognizer(nil)
    writeAnswer(out, ["speech": speechState(), "on_device": rec?.supportsOnDeviceRecognition ?? false,
                      "locale": rec?.locale.identifier ?? Locale.current.identifier])
}

func transcribe(audio: String, out: String, language: String?) {
    let locale = language ?? Locale.current.identifier
    let state = speechState()
    guard state == "granted" else {                  // a task never prompts: `ferb setup` asks once
        writeAnswer(out, ["error": "Speech Recognition isn't allowed for FERB", "reason": "not_authorized",
                          "speech": state, "locale": locale]); return }
    guard let rec = recognizer(language) else {
        writeAnswer(out, ["error": "no speech recognizer for \(locale)", "reason": "unsupported_language",
                          "locale": locale]); return }
    guard rec.supportsOnDeviceRecognition else {     // no cloud fallback: the sound never leaves the Mac
        writeAnswer(out, ["error": "on-device recognition isn't available for \(rec.locale.identifier)",
                          "reason": "on_device_unavailable", "locale": rec.locale.identifier]); return }
    guard rec.isAvailable else {
        writeAnswer(out, ["error": "speech recognition isn't available right now", "reason": "unavailable",
                          "locale": rec.locale.identifier]); return }
    rec.queue = OperationQueue()                     // answers off the main thread, which waits below
    let req = SFSpeechURLRecognitionRequest(url: URL(fileURLWithPath: audio))
    req.requiresOnDeviceRecognition = true
    req.shouldReportPartialResults = false
    req.addsPunctuation = true
    let sem = DispatchSemaphore(value: 0)
    let lock = NSLock()
    var answer: [String: Any]? = nil
    let task = rec.recognitionTask(with: req) { r, err in
        lock.lock(); defer { lock.unlock() }
        if answer != nil { return }
        if let r = r, r.isFinal {
            let best = r.bestTranscription
            answer = ["on_device": true, "locale": rec.locale.identifier, "text": best.formattedString,
                      "segments": best.segments.map { ["text": $0.substring, "start": $0.timestamp, "duration": $0.duration] }]
            sem.signal()
        } else if let err = err as NSError? {
            if err.domain == "kAFAssistantErrorDomain" && err.code == 1110 {   // no speech in the sound
                answer = ["on_device": true, "locale": rec.locale.identifier, "text": "", "segments": []]
            } else {
                answer = ["error": err.localizedDescription, "reason": "failed", "locale": rec.locale.identifier]
            }
            sem.signal()
        }
    }
    sem.wait()
    _ = task
    writeAnswer(out, answer ?? ["error": "no answer", "reason": "failed"])
}

// ---------- the service ----------

func serve() {
    let listener = Listener()
    listener.warmUp()
    while true {
        guard let conn = connectAsVoice() else { Thread.sleep(forTimeInterval: 3); continue }
        let devices = inputDevices().map { ["uid": $0.uid, "name": $0.name, "transport": $0.transport, "default": $0.isDefault] as [String: Any] }
        guard conn.send(["jsonrpc": "2.0", "id": 2, "method": "voice.attach",
                         "params": ["devices": devices, "permissions": permissionStates()]]) else { Thread.sleep(forTimeInterval: 3); continue }
        while let msg = conn.readLine() {
            guard msg["method"] as? String == "voice.attach.event", let cmd = msg["params"] as? [String: Any] else {
                if msg["id"] as? Int == 2 { break }                  // the stream ended
                continue }
            switch cmd["op"] as? String {
            case "start":
                if let err = listener.start(uid: cmd["device_uid"] as? String ?? "") {
                    var failed: [String: Any] = ["text": "", "error": err]
                    if err == ON_DEVICE_UNAVAILABLE { failed["on_device"] = false }
                    _ = connectAsVoice()?.call(3, "voice.transcript", failed) }
            case "chime":
                NSSound(named: "Tink")?.play()
            case "stop":
                let text = listener.stop(sound: cmd["sound"] as? Bool ?? true)
                _ = connectAsVoice()?.call(3, "voice.transcript", ["text": text])
            case "discard":
                listener.discard()
            default: break
            }
        }
        listener.discard()
        Thread.sleep(forTimeInterval: 2)                              // the daemon restarted: connect again
    }
}

let args = Array(CommandLine.arguments.dropFirst())
if args.first == "request" {
    NSApplication.shared.setActivationPolicy(.accessory)
    requestPermissions()
    exit(0)
}
if args.first == "request-speech" {
    NSApplication.shared.setActivationPolicy(.accessory)
    requestSpeech()
    exit(0)
}
if args.first == "transcribe", args.count >= 3 {
    transcribe(audio: args[1], out: args[2], language: args.count > 3 ? args[3] : nil)
    exit(0)
}
if args.first == "status", args.count >= 2 {
    writeStatus(args[1])
    exit(0)
}
NSApplication.shared.setActivationPolicy(.accessory)
DispatchQueue.global().async { serve() }
NSApplication.shared.run()
