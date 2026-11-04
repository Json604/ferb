// FERB.app's hearing (L26): on-device speech recognition of sound a task captured from an app. The same executable as
// the notch; the daemon starts it per use with `open -g -n -W --env FERB_HOME=… FERB.app --args <mode> …`
// (LaunchServices: FERB.app, not the daemon, is the app macOS asks about). main() runs a hearing mode before
// NSApplication, the delegate, Sparkle, login items, the daemon connection or any window, and exits; no microphone is
// opened (push-to-talk is the app's own dictation, VOICE-409). Speech is recognized on this Mac only
// (requiresOnDeviceRecognition): without on-device support FERB says so and never sends speech to Apple's servers
// (rule 15, L26).
// Modes: request-speech                 ask for Speech Recognition only, print the result (kept and harmless; setup no
//                                       longer calls it: the app asks with its microphone row)
//        transcribe <audio> <out> [locale]  a captured app's sound file to text, written into <out>
//        status <out>                   Speech Recognition state and on-device support, written into <out>
// <out> is the daemon's empty answer file in $FERB_HOME/workspace/audio; deleting it stops FERB.app. The two never
// prompt and write nothing to stdout or any log. FERB_HOME is taken only unset or as the account's own ~/.ferb
// (FerbHomeRule.swift); every mode checks it first. Any other first argument starts the normal app.
import AppKit
import NotchCore
import Speech

enum Hearing {
    static let modes: Set<String> = ["request-speech", "transcribe", "status"]

    /// The lease watch; its handler ends the process (tests replace it).
    nonisolated(unsafe) static var onLeaseLost: @Sendable () -> Void = { exit(0) }
    nonisolated(unsafe) private static var leaseWatch: DispatchSourceFileSystemObject?

    /// A hearing mode's exit code once it ran, or nil when `args` (CommandLine.arguments) names no mode: the normal app.
    static func run(_ args: [String], givenHome: String? = getenv("FERB_HOME").map { String(cString: $0) },
                    account: String? = accountFerbHome()) -> Int32? {
        guard let mode = args.dropFirst().first, modes.contains(mode) else { return nil }
        let rest = Array(args.dropFirst(2))
        let home: String
        switch resolveFerbHome(givenHome, account: account) {     // a foreign FERB_HOME is refused before anything else
        case .home(let ok): home = ok
        case .refused(let why):
            fputs(why + "\n", stderr)
            return 64
        }
        switch mode {
        case "request-speech":
            NSApplication.shared.setActivationPolicy(.accessory)
            requestSpeech()
            return 0
        case "transcribe" where rest.count >= 2:
            transcribe(audio: rest[0], out: rest[1], language: rest.count > 2 ? rest[2] : nil, home: home)
            return 0
        case "status" where rest.count >= 1:
            writeStatus(rest[0], home: home)
            return 0
        default:
            return 64                                             // a mode without its arguments: nothing to do
        }
    }

    // ---------- permissions ----------

    static func speechState() -> String {
        switch SFSpeechRecognizer.authorizationStatus() {
        case .authorized: return "granted"; case .denied: return "denied"; case .restricted: return "restricted"
        default: return "not_determined" }
    }

    private static func requestSpeech() {                         // Speech Recognition only, never the microphone
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
    static func answerFile(_ out: String, home: String) -> Int32? {
        guard inAudioFolder(out, home: home) else { return nil }
        let fd = open(out, O_WRONLY | O_NOFOLLOW)              // never creates a file: only the daemon's own
        if fd < 0 { return nil }
        var st = stat()
        guard fstat(fd, &st) == 0, (st.st_mode & S_IFMT) == S_IFREG, st.st_size == 0, st.st_uid == getuid(),
              st.st_nlink == 1 else {                          // no second name for it, no existing answer replaced
            close(fd); return nil }
        let watch = DispatchSource.makeFileSystemObjectSource(fileDescriptor: fd, eventMask: [.delete, .rename, .revoke],
                                                              queue: .global())
        watch.setEventHandler { onLeaseLost() }                // stopped: no answer, recognition ends with the process
        watch.setCancelHandler { close(fd) }                   // the watched descriptor closes only once unwatched
        watch.resume()
        leaseWatch = watch
        return fd
    }

    // FERB's private audio folder: $FERB_HOME/workspace/audio (the daemon passes FERB_HOME with `open --env`; the home
    // rule takes it only as the account's own ~/.ferb).
    static func inAudioFolder(_ path: String, home: String) -> Bool {
        guard let folder = realpath(home + "/workspace/audio", nil) else { return false }
        defer { free(folder) }
        guard let parent = realpath((path as NSString).deletingLastPathComponent, nil) else { return false }
        defer { free(parent) }
        return String(cString: parent) == String(cString: folder)
    }

    static func writeAnswer(_ fd: Int32, _ answer: [String: Any]) {
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

    private static func recognizer(_ language: String?) -> SFSpeechRecognizer? {
        if let language = language { return SFSpeechRecognizer(locale: Locale(identifier: language)) }
        return SFSpeechRecognizer()                       // the Mac's own language
    }

    static func writeStatus(_ out: String, home: String) {
        guard let fd = answerFile(out, home: home) else { return }
        let rec = recognizer(nil)
        writeAnswer(fd, ["speech": speechState(), "on_device": rec?.supportsOnDeviceRecognition ?? false,
                         "locale": rec?.locale.identifier ?? Locale.current.identifier])
    }

    static func transcribe(audio: String, out: String, language: String?, home: String) {
        guard let fd = answerFile(out, home: home) else { return }   // not the daemon's answer file: do nothing
        let locale = language ?? Locale.current.identifier
        guard inAudioFolder(audio, home: home) else {
            writeAnswer(fd, ["error": "the sound file isn't in FERB's audio folder", "reason": "failed"]); return }
        let state = speechState()
        guard state == "granted" else {                  // a task never prompts: FERB's setup asks once
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
        nonisolated(unsafe) var answer: [String: Any]? = nil
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
}
