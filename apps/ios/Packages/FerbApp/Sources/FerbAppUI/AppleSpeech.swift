import AVFoundation
import FerbAppModel
@preconcurrency import Speech

/// Apple's speech recogniser behind `VoiceInput` (kb PHN-604; FERB.app's Microphone.swift on the phone). Where this
/// iPhone recognises the language on the phone itself (`supportsOnDeviceRecognition`) every request requires it
/// (`requiresOnDeviceRecognition`) and nothing leaves the phone. Where it can't, Apple's speech service is used: it may
/// send the sound of the voice to Apple. The owner is told which before his first use of each (`.voice`). The microphone
/// runs only while he holds; its buffers go to the request in memory and nowhere else: nothing is written, kept or sent
/// to the Mac.
@MainActor
final class AppleSpeech: VoiceInput {
    private let engine = AVAudioEngine()
    private var recognizer: SFSpeechRecognizer?
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var recognition: SFSpeechRecognitionTask?
    private let feed = Feed()
    private var running = false

    /// The tap's side, on AVFAudio's audio thread: the request now taking the sound (swapped when a task is replaced).
    final class Feed: @unchecked Sendable {
        private let lock = NSLock()
        private var request: SFSpeechAudioBufferRecognitionRequest?
        private var heard: (@Sendable (Float) -> Void)?
        private var lastLevel = Date.distantPast
        func set(_ r: SFSpeechAudioBufferRecognitionRequest?) { lock.lock(); request = r; lock.unlock() }
        /// How loud each buffer is (kb PHN-643), at most about 15 times a second; never the sound itself.
        func onLevel(_ f: (@Sendable (Float) -> Void)?) { lock.lock(); heard = f; lock.unlock() }
        func receive(_ buffer: AVAudioPCMBuffer) {
            lock.lock()
            let r = request, f = heard
            let due = Date().timeIntervalSince(lastLevel) > 0.065
            if due { lastLevel = Date() }
            lock.unlock()
            r?.append(buffer)
            if due, let f, let samples = buffer.floatChannelData?[0], buffer.frameLength > 0 {
                var sum: Float = 0
                for i in 0..<Int(buffer.frameLength) { sum += samples[i] * samples[i] }
                let db = 10 * log10(max(sum / Float(buffer.frameLength), 1e-10))
                f(min(max((db + 50) / 40, 0), 1))   // about -50 dB (quiet room) to -10 dB (speech close by)
            }
        }
    }

    /// Both of iOS's questions were answered yes: the microphone and speech recognition.
    static var allowed: Bool {
        AVAudioApplication.shared.recordPermission == .granted && SFSpeechRecognizer.authorizationStatus() == .authorized
    }

    /// iOS's two questions, each only if it isn't answered; true when both are yes.
    static func ask() async -> Bool {
        let mic = await AVAudioApplication.requestRecordPermission()
        guard mic else { return false }
        let speech = await withCheckedContinuation { (c: CheckedContinuation<SFSpeechRecognizerAuthorizationStatus, Never>) in
            SFSpeechRecognizer.requestAuthorization { c.resume(returning: $0) }
        }
        return speech == .authorized
    }

    func kind() -> SpeechKind {
        guard let r = SFSpeechRecognizer(locale: .current), r.isAvailable else { return .none }
        return r.supportsOnDeviceRecognition ? .onDevice : .service
    }

    func startTask(kind: SpeechKind, task: Int, events: @escaping @MainActor (VoiceEvent) -> Void) {
        if recognizer == nil { recognizer = SFSpeechRecognizer(locale: .current) }
        guard let recognizer, recognizer.isAvailable else {
            events(.failed(task: task, noSpeech: false, languageMissing: false, code: -1))
            return
        }
        if kind == .onDevice && !recognizer.supportsOnDeviceRecognition {
            // the phone was told on-device and can't now: never fall back to Apple's service without the owner's word
            events(.failed(task: task, noSpeech: false, languageMissing: true, code: -2))
            return
        }
        let r = SFSpeechAudioBufferRecognitionRequest()
        r.requiresOnDeviceRecognition = kind == .onDevice
        r.shouldReportPartialResults = true
        r.addsPunctuation = true
        request?.endAudio()
        request = r
        feed.set(r)
        recognition = recognizer.recognitionTask(with: r) { result, error in
            let heard = result.map { res -> RecognizedSpeech in
                let best = res.bestTranscription
                let first = best.segments.first, last = best.segments.last
                return RecognizedSpeech(best.formattedString, isFinal: res.isFinal, utteranceEnded: res.speechRecognitionMetadata != nil,
                                        start: first.map(\.timestamp), end: last.map { $0.timestamp + $0.duration })
            }
            let ns = error as NSError?
            // "no speech detected" (the same fact the Mac reads)
            let noSpeech = ns.map { $0.domain == "kAFAssistantErrorDomain" && $0.code == 1110 } ?? false
            Task { @MainActor in
                if let heard { events(.heard(task: task, heard)) }
                if heard?.isFinal != true, let ns { events(.failed(task: task, noSpeech: noSpeech, languageMissing: false, code: ns.code)) }
            }
        }
        feed.onLevel { l in Task { @MainActor in events(.level(task: task, l)) } }
        if !running { startMicrophone(task: task, events: events) }
        // the sound goes to this task's request from now: FERB may say "Listening…" (kb PHN-643)
        if running { events(.ready(task: task)) }
    }

    private func startMicrophone(task: Int, events: @escaping @MainActor (VoiceEvent) -> Void) {
        do {
            let session = AVAudioSession.sharedInstance()
            try session.setCategory(.record, mode: .measurement, options: .duckOthers)
            try session.setActive(true, options: .notifyOthersOnDeactivation)
            let input = engine.inputNode
            let feed = self.feed
            input.installTap(onBus: 0, bufferSize: 1024, format: input.outputFormat(forBus: 0)) { buffer, _ in feed.receive(buffer) }
            engine.prepare()
            try engine.start()
            running = true
        } catch {
            stop()
            // the microphone couldn't start: another app holds it, or a call is on
            events(.failed(task: task, noSpeech: false, languageMissing: false, code: AppModel.micStartFailed))
        }
    }

    func finishAudio() {
        stopMicrophone()
        feed.set(nil)
        request?.endAudio()
    }

    func stop() {
        stopMicrophone()
        feed.set(nil)
        feed.onLevel(nil)
        request?.endAudio()
        request = nil
        recognition?.cancel()
        recognition = nil
        recognizer = nil
    }

    private func stopMicrophone() {
        guard running else { return }
        engine.stop()
        engine.inputNode.removeTap(onBus: 0)
        running = false
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }
}
