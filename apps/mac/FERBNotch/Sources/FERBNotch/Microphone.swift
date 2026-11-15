import AVFoundation
import AudioCapture
import NotchCore
@preconcurrency import Speech

/// Dictation is recognized on this Mac only (Task 20 / L26 ruling, final review M5): raw voice is unredacted, so it never
/// goes to Apple's servers. Without on-device recognition for the current language there is no request, and FERB says
/// so instead of listening.
enum OnDeviceSpeech {
    static let unavailable = "Dictation needs on-device speech recognition for your language, and it isn't available "
        + "on this Mac, so FERB doesn't listen (your voice never goes to Apple's servers). Please type your request."

    static func request(supportsOnDevice: Bool) -> SFSpeechAudioBufferRecognitionRequest? {
        guard supportsOnDevice else { return nil }
        let request = SFSpeechAudioBufferRecognitionRequest()
        request.requiresOnDeviceRecognition = true
        request.shouldReportPartialResults = true
        request.addsPunctuation = true
        return request
    }
}

@MainActor
protocol VoiceCapturing: AnyObject {
    /// `update`: everything said so far. `level`: the input's RMS per block of `duration` seconds, as it arrives.
    func start(update: @escaping (String) -> Void, level: @escaping ([Double], TimeInterval) -> Void,
               completion: @escaping (Result<String, Error>) -> Void)
    func finish()
    func cancel()
}

/// The tap's side of capture, on AVFAudio's audio thread: the recognition request now taking the audio (swapped when
/// a recognition task is replaced, so no buffer is lost between tasks) and the input's loudness for the meter.
final class MicrophoneFeed: @unchecked Sendable {
    private let lock = NSLock()
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private let levels: @Sendable ([Double], TimeInterval) -> Void

    init(levels: @escaping @Sendable ([Double], TimeInterval) -> Void) { self.levels = levels }

    func feed(_ request: SFSpeechAudioBufferRecognitionRequest?) {
        lock.lock(); self.request = request; lock.unlock()
    }

    func receive(_ buffer: AVAudioPCMBuffer) {
        lock.lock(); let request = self.request; lock.unlock()
        request?.append(buffer)
        // The meter reads 20 ms blocks of the first channel, whatever buffer size the hardware delivers.
        let rate = buffer.format.sampleRate, frames = Int(buffer.frameLength)
        guard rate > 0, frames > 0, let channel = buffer.floatChannelData?[0] else { return }
        let chunk = max(1, Int(rate / 50))
        let values = VoiceLevel.rms(UnsafeBufferPointer(start: channel, count: frames), chunk: chunk)
        levels(values, Double(chunk) / rate)
    }
}

@MainActor
final class Microphone: VoiceCapturing {
    private let engine = AVAudioEngine()
    private var routeObserver: NSObjectProtocol?
    private let routeRecovery = MicrophoneRouteRecovery()
    private var recognizer: SFSpeechRecognizer?
    private var updateTranscript: ((String) -> Void)?
    private var updateLevel: (([Double], TimeInterval) -> Void)?
    /// Everything said in this dictation, and which recognition task takes the audio (APP-673).
    private var dictation = DictationCapture()
    private var feed: MicrophoneFeed?
    private var captureStarted = false
    private var isFinishing = false
    private var recognition: SFSpeechRecognitionTask?
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var permissionTask: Task<Void, Never>?
    private var finishingTask: Task<Void, Never>?
    private var completion: ((Result<String, Error>) -> Void)?
    private var generation = UUID()
    private var hasTap = false

    init() {
        routeObserver = NotificationCenter.default.addObserver(
            forName: .AVAudioEngineConfigurationChange, object: engine, queue: nil
        ) { [weak self] _ in
            // Never rebuild on Core Audio's internal notification queue.
            Task { @MainActor in self?.audioConfigurationChanged() }
        }
    }

    deinit { if let routeObserver { NotificationCenter.default.removeObserver(routeObserver) } }

    private func audioConfigurationChanged() {
        guard captureStarted, !isFinishing, completion != nil, !engine.isRunning else { return }
        stopCapture()
        // The old task gets the end of its audio and may still revise its words; they stay in its own slot.
        dictation.leaveTask()
        feed?.feed(nil)
        request?.endAudio(); request = nil
        recognition = nil
        let token = generation
        routeRecovery.schedule { [weak self] in
            guard let self, self.generation == token, !self.isFinishing, self.captureStarted else { return }
            if let error = FERBResetMicrophoneEngine(self.engine) {
                self.complete(.failure(error))
                return
            }
            NSLog("FERB voice: audio configuration changed; rebuilding microphone capture.")
            self.runCapture(token: token)
        }
    }

    func start(update: @escaping (String) -> Void, level: @escaping ([Double], TimeInterval) -> Void,
               completion: @escaping (Result<String, Error>) -> Void) {
        cancel()
        self.completion = completion
        updateTranscript = update
        updateLevel = level
        let token = generation
        permissionTask = Task { [weak self] in
            let microphone = await AVCaptureDevice.requestAccess(for: .audio)
            guard let self, self.generation == token, !Task.isCancelled else { return }
            guard microphone else { self.fail("Allow Microphone access for \(AppName.current) in System Settings → Privacy & Security."); return }
            let speech = await withCheckedContinuation { continuation in
                SFSpeechRecognizer.requestAuthorization { continuation.resume(returning: $0) }
            }
            guard self.generation == token, !Task.isCancelled else { return }
            guard speech == .authorized else { self.fail("Allow Speech Recognition for \(AppName.current) in System Settings → Privacy & Security."); return }
            self.beginCapture(token: token)
        }
    }

    private func beginCapture(token: UUID) {
        guard let recognizer = SFSpeechRecognizer(locale: .current), recognizer.isAvailable else {
            fail("Speech recognition is unavailable for your current language. Try again or type your request.")
            return
        }
        // Checked before the microphone is tapped: no audio is captured for a request that would leave the Mac.
        guard recognizer.supportsOnDeviceRecognition else {
            fail(OnDeviceSpeech.unavailable)
            return
        }
        self.recognizer = recognizer
        captureStarted = true
        // AVFAudio calls the feed on its audio thread; the loudness hops to the main actor, for this dictation only.
        feed = MicrophoneFeed(levels: { [weak self] values, duration in
            Task { @MainActor in
                guard let self, self.generation == token else { return }
                self.updateLevel?(values, duration)
            }
        })
        runCapture(token: token)
    }

    /// A recognition task, the tap and the engine: at the start and after an audio route change.
    private func runCapture(token: UUID) {
        guard let feed, startRecognition(token: token) else { return }
        // AVFAudio invokes this on its audio thread; it must not inherit MainActor.
        if let error = FERBInstallMicrophoneTap(engine, { @Sendable buffer, _ in feed.receive(buffer) }) {
            complete(.failure(error))
            return
        }
        hasTap = true
        if let error = FERBStartMicrophoneEngine(engine) { complete(.failure(error)) }
    }

    /// A new recognition task takes the audio from the next buffer on; earlier tasks keep their words.
    @discardableResult
    private func startRecognition(token: UUID) -> Bool {
        guard let recognizer,
              let request = OnDeviceSpeech.request(supportsOnDevice: recognizer.supportsOnDeviceRecognition) else {
            fail(OnDeviceSpeech.unavailable)
            return false
        }
        self.request?.endAudio()
        let task = dictation.beginTask()
        self.request = request
        feed?.feed(request)
        recognition = recognizer.recognitionTask(with: request) { [weak self] result, error in
            let heard = result.map(Self.recognized)
            let noSpeech = (error as NSError?).map(Self.isNoSpeech) ?? false
            Task { @MainActor in
                guard let self, self.generation == token else { return }
                self.answered(task: task, heard: heard, error: error, noSpeech: noSpeech, token: token)
            }
        }
        return true
    }

    private func answered(task: Int, heard: RecognizedSpeech?, error: Error?, noSpeech: Bool, token: UUID) {
        var step = DictationCapture.Step.keepListening
        if let heard {
            step = dictation.receive(heard, task: task)
            updateTranscript?(dictation.text)
        }
        if heard?.isFinal != true, let error {
            let nsError = error as NSError
            step = dictation.fail(task: task, noSpeech: noSpeech)
            if step != .keepListening {
                NSLog("FERB voice: recognition ended (%@ %ld).", nsError.domain, nsError.code)
            }
        }
        switch step {
        case .keepListening: break
        case .restart:
            // The recognizer ended while the owner is still dictating: a new task on the running microphone.
            guard captureStarted, !isFinishing, hasTap else { return }
            NSLog("FERB voice: recognition ended mid-dictation; continuing with a new recognition task.")
            startRecognition(token: token)
        case .finished(let text): complete(.success(text))
        case .failed:
            complete(.failure(error ?? NSError(domain: "FERBVoice", code: 2, userInfo: [
                NSLocalizedDescriptionKey: "Dictation stopped unexpectedly. Try again or type your request."])))
        }
    }

    nonisolated private static func recognized(_ result: SFSpeechRecognitionResult) -> RecognizedSpeech {
        let best = result.bestTranscription
        let first = best.segments.first, last = best.segments.last
        return RecognizedSpeech(best.formattedString, isFinal: result.isFinal,
                                utteranceEnded: result.speechRecognitionMetadata != nil,
                                start: first.map(\.timestamp), end: last.map { $0.timestamp + $0.duration })
    }

    /// Apple Speech's "no speech detected": the recognizer heard nothing in its audio (the same fact hearing reads).
    nonisolated private static func isNoSpeech(_ error: NSError) -> Bool {
        error.domain == "kAFAssistantErrorDomain" && error.code == 1110
    }

    func finish() {
        isFinishing = true
        routeRecovery.cancel()
        // Releasing the chord during a permission prompt must never start recording later.
        guard captureStarted else { complete(.success(dictation.text)); return }
        stopCapture()
        feed?.feed(nil)
        request?.endAudio()
        // The current task's last answer completes the dictation; without one, what was said so far does.
        if case .finished(let text) = dictation.finish() { complete(.success(text)); return }
        finishingTask = Task { [weak self] in
            do { try await Task.sleep(for: .seconds(2)) } catch { return }
            guard let self else { return }
            self.complete(.success(self.dictation.text))
        }
    }

    func cancel() {
        generation = UUID()
        captureStarted = false
        isFinishing = false
        routeRecovery.cancel()
        updateTranscript = nil
        updateLevel = nil
        recognizer = nil
        permissionTask?.cancel(); permissionTask = nil
        finishingTask?.cancel(); finishingTask = nil
        stopCapture()
        feed?.feed(nil); feed = nil
        request?.endAudio(); request = nil
        recognition?.cancel(); recognition = nil
        completion = nil
        dictation = DictationCapture()
    }

    private func stopCapture() {
        if FERBStopMicrophoneEngine(engine, hasTap) != nil {
            NSLog("FERB voice: microphone cleanup encountered an audio device change.")
        }
        hasTap = false
    }

    private func complete(_ result: Result<String, Error>) {
        let callback = completion
        cancel()
        callback?(result)
    }

    private func fail(_ message: String) {
        complete(.failure(NSError(domain: "FERBVoice", code: 1, userInfo: [NSLocalizedDescriptionKey: message])))
    }
}

/// The dictation meter's state for the notch (APP-674): only the meter view observes it, so the microphone's ~50
/// blocks a second never redraw the rest of the notch.
@MainActor
final class VoiceMeter: ObservableObject {
    @Published private(set) var value = VoiceLevel()

    func add(_ rms: [Double], each duration: TimeInterval) {
        guard !rms.isEmpty else { return }
        var next = value
        for block in rms { next.add(rms: block, duration: duration) }
        value = next
    }

    func reset() { value = VoiceLevel() }
}
