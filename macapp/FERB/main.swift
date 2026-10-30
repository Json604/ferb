// FERB.app's voice helper (FERB Voice): FERB's on-device speech recognition for hearing (L26). No windows, Dock icon
// or menu-bar item, and no microphone: it only reads sound files the daemon captured from an app. Speech is
// recognized on this Mac only (requiresOnDeviceRecognition): without on-device support FERB says so and never sends
// speech to Apple's servers (rule 15, L26). Push-to-talk is the FERB app's own dictation (notes/voice.md decision 3).
// Usage: FERB request-speech                ask for Speech Recognition only, print the result (setup)
//        FERB transcribe <audio> <out> [locale]  L26: a captured app's sound file to text, written into <out>
//        FERB status <out>                  Speech Recognition state and on-device support, written into <out>
// The daemon starts the last two per use with `open -g -n -W --env FERB_HOME=…` (LaunchServices: FERB.app, not the
// daemon, is the app macOS asks about). <out> is the daemon's empty answer file in $FERB_HOME/workspace/audio; deleting
// it stops FERB.app. They never prompt and write nothing to stdout or any log. Started with no mode, it exits.
// FERB_HOME is taken only when unset or when it is the account's own ~/.ferb (home.swift); every mode checks it first.
import AppKit
import Speech

_ = ferbHome                                          // a foreign FERB_HOME is refused before anything else (home.swift)

// ---------- permissions ----------

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

// ---------- hearing (L26): a captured app's sound to text, on this Mac only ----------

// The answer file is the daemon's lease. It creates the file empty (0600) in FERB's audio folder before launching
// FERB.app; FERB.app only ever writes into such a file there, never creating or replacing one. Deleting it is the
// daemon's stop: FERB.app exits at once without answering, and anything written after the deletion would land in an
// unlinked file, so no transcript is left on disk.
var leaseWatch: DispatchSourceFileSystemObject? = nil

func answerFile(_ out: String) -> Int32? {
    guard inAudioFolder(out) else { return nil }
    let fd = open(out, O_WRONLY | O_NOFOLLOW)              // never creates a file: only the daemon's own
    if fd < 0 { return nil }
    var st = stat()
    guard fstat(fd, &st) == 0, (st.st_mode & S_IFMT) == S_IFREG, st.st_size == 0, st.st_uid == getuid(),
          st.st_nlink == 1 else {                          // no second name for it, no existing answer replaced
        close(fd); return nil }
    let watch = DispatchSource.makeFileSystemObjectSource(fileDescriptor: fd, eventMask: [.delete, .rename, .revoke],
                                                          queue: .global())
    watch.setEventHandler { exit(0) }                      // stopped: no answer, recognition ends with the process
    watch.setCancelHandler { close(fd) }                   // the watched descriptor closes only once unwatched
    watch.resume()
    leaseWatch = watch
    return fd
}

// FERB's private audio folder: $FERB_HOME/workspace/audio (the daemon passes FERB_HOME with `open --env`; home.swift
// takes it only as the account's own ~/.ferb).
func inAudioFolder(_ path: String) -> Bool {
    guard let folder = realpath(ferbHome + "/workspace/audio", nil) else { return false }
    defer { free(folder) }
    guard let parent = realpath((path as NSString).deletingLastPathComponent, nil) else { return false }
    defer { free(parent) }
    return String(cString: parent) == String(cString: folder)
}

func writeAnswer(_ fd: Int32, _ answer: [String: Any]) {
    if let data = try? JSONSerialization.data(withJSONObject: answer) {
        _ = data.withUnsafeBytes { write(fd, $0.baseAddress, data.count) }
    }
    if let watch = leaseWatch {                            // unwatch first; its cancel handler closes the descriptor
        leaseWatch = nil
        watch.cancel()
    } else {
        close(fd)
    }
}

func recognizer(_ language: String?) -> SFSpeechRecognizer? {
    if let language = language { return SFSpeechRecognizer(locale: Locale(identifier: language)) }
    return SFSpeechRecognizer()                       // the Mac's own language
}

func writeStatus(_ out: String) {
    guard let fd = answerFile(out) else { return }
    let rec = recognizer(nil)
    writeAnswer(fd, ["speech": speechState(), "on_device": rec?.supportsOnDeviceRecognition ?? false,
                     "locale": rec?.locale.identifier ?? Locale.current.identifier])
}

func transcribe(audio: String, out: String, language: String?) {
    guard let fd = answerFile(out) else { return }   // not the daemon's answer file: do nothing
    let locale = language ?? Locale.current.identifier
    guard inAudioFolder(audio) else {
        writeAnswer(fd, ["error": "the sound file isn't in FERB's audio folder", "reason": "failed"]); return }
    let state = speechState()
    guard state == "granted" else {                  // a task never prompts: `ferb setup` asks once
        writeAnswer(fd, ["error": "Speech Recognition isn't allowed for FERB", "reason": "not_authorized",
                          "speech": state, "locale": locale]); return }
    guard let rec = recognizer(language) else {
        writeAnswer(fd, ["error": "no speech recognizer for \(locale)", "reason": "unsupported_language",
                          "locale": locale]); return }
    guard rec.supportsOnDeviceRecognition else {     // no cloud fallback: the sound never leaves the Mac
        writeAnswer(fd, ["error": "on-device recognition isn't available for \(rec.locale.identifier)",
                          "reason": "on_device_unavailable", "locale": rec.locale.identifier]); return }
    guard rec.isAvailable else {
        writeAnswer(fd, ["error": "speech recognition isn't available right now", "reason": "unavailable",
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
    writeAnswer(fd, answer ?? ["error": "no answer", "reason": "failed"])
}

let args = Array(CommandLine.arguments.dropFirst())
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
exit(64)                                              // no mode: nothing to do (push-to-talk is the app's dictation)
