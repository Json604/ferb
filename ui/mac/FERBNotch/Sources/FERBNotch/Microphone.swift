import AVFoundation
import AudioCapture
@preconcurrency import Speech

@MainActor
protocol VoiceCapturing: AnyObject {
    func start(update: @escaping (String) -> Void, completion: @escaping (Result<String, Error>) -> Void)
    func finish()
    func cancel()
}

@MainActor
final class Microphone: VoiceCapturing {
    private let engine = AVAudioEngine()
    private var routeObserver: NSObjectProtocol?
    private let routeRecovery = MicrophoneRouteRecovery()
    private var recognizer: SFSpeechRecognizer?
    private var recognitionRevision = UUID()
    private var updateTranscript: ((String) -> Void)?
    private var captureStarted = false
    private var isFinishing = false
    private var recognition: SFSpeechRecognitionTask?
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var permissionTask: Task<Void, Never>?
    private var finishingTask: Task<Void, Never>?
    private var completion: ((Result<String, Error>) -> Void)?
    private var generation = UUID()
    private var transcript = ""
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
        recognitionRevision = UUID()
        stopCapture()
        request?.endAudio(); request = nil
        recognition?.cancel(); recognition = nil
        let token = generation
        routeRecovery.schedule { [weak self] in
            guard let self, self.generation == token, !self.isFinishing,
                  self.captureStarted, let update = self.updateTranscript else { return }
            if let error = FERBResetMicrophoneEngine(self.engine) {
                self.complete(.failure(error))
                return
            }
            NSLog("FERB voice: audio configuration changed; rebuilding microphone capture.")
            self.capture(token: token, update: update)
        }
    }

    func start(update: @escaping (String) -> Void, completion: @escaping (Result<String, Error>) -> Void) {
        cancel()
        self.completion = completion
        updateTranscript = update
        let token = generation
        permissionTask = Task { [weak self] in
            let microphone = await AVCaptureDevice.requestAccess(for: .audio)
            guard let self, self.generation == token, !Task.isCancelled else { return }
            guard microphone else { self.fail("Allow Microphone access for FERB Notch in System Settings → Privacy & Security."); return }
            let speech = await withCheckedContinuation { continuation in
                SFSpeechRecognizer.requestAuthorization { continuation.resume(returning: $0) }
            }
            guard self.generation == token, !Task.isCancelled else { return }
            guard speech == .authorized else { self.fail("Allow Speech Recognition for FERB Notch in System Settings → Privacy & Security."); return }
            self.capture(token: token, update: update)
        }
    }

    private func capture(token: UUID, update: @escaping (String) -> Void) {
        guard let recognizer = SFSpeechRecognizer(locale: .current), recognizer.isAvailable else {
            fail("Speech recognition is unavailable for your current language. Try again or type your request.")
            return
        }
        self.recognizer = recognizer
        captureStarted = true
        recognitionRevision = UUID()
        let revision = recognitionRevision
        let prefix = transcript.isEmpty ? "" : transcript + " "
        let request = SFSpeechAudioBufferRecognitionRequest()
        request.shouldReportPartialResults = true
        request.addsPunctuation = true
        if recognizer.supportsOnDeviceRecognition { request.requiresOnDeviceRecognition = true }
        self.request = request
        // AVFAudio invokes this on its audio thread; it must not inherit MainActor.
        if let error = FERBInstallMicrophoneTap(engine, { @Sendable buffer, _ in request.append(buffer) }) {
            complete(.failure(error))
            return
        }
        hasTap = true
        recognition = recognizer.recognitionTask(with: request) { [weak self] result, error in
            let text = result?.bestTranscription.formattedString
            let final = result?.isFinal == true
            Task { @MainActor in
                guard let self, self.generation == token, self.recognitionRevision == revision else { return }
                if let text { self.transcript = prefix + text; update(self.transcript) }
                if final { self.complete(.success(self.transcript)) }
                else if let error {
                    if !self.transcript.isEmpty { self.complete(.success(self.transcript)) }
                    else { self.complete(.failure(error)) }
                }
            }
        }
        if let error = FERBStartMicrophoneEngine(engine) { complete(.failure(error)) }
    }

    func finish() {
        isFinishing = true
        routeRecovery.cancel()
        // Releasing the chord during a permission prompt must never start recording later.
        guard request != nil else { complete(.success(transcript)); return }
        stopCapture()
        request?.endAudio()
        finishingTask = Task { [weak self] in
            do { try await Task.sleep(for: .seconds(2)) } catch { return }
            guard let self else { return }
            self.complete(.success(self.transcript))
        }
    }

    func cancel() {
        generation = UUID()
        recognitionRevision = UUID()
        captureStarted = false
        isFinishing = false
        routeRecovery.cancel()
        updateTranscript = nil
        recognizer = nil
        permissionTask?.cancel(); permissionTask = nil
        finishingTask?.cancel(); finishingTask = nil
        stopCapture()
        request?.endAudio(); request = nil
        recognition?.cancel(); recognition = nil
        completion = nil
        transcript = ""
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
