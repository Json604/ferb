import Foundation

// Speaking to FERB from the phone (kb PHN-603..PHN-605): hold the microphone in the composer and speak; the words appear
// in the message box as they are heard and stay there when the owner lets go, for him to check and send. Never sent by
// itself. This file is the part with no iOS in it: FERB.app's own dictation model (NotchCore Dictation.swift, APP-673
// and APP-675), and the controller that runs one dictation over the phone's recogniser. Android's twin is
// model/Dictation.kt; both run apps/android/app/src/test/resources/dictation-cases.json.

/// One answer of the speech recogniser, as plain facts.
public struct RecognizedSpeech: Equatable, Sendable {
    public var text: String
    /// The recognition task is done: nothing more comes from it.
    public var isFinal: Bool
    /// The recogniser marked the end of an utterance: its later answers can start again without these words.
    public var utteranceEnded: Bool
    /// Where the first word starts and the last ends, in seconds of the task's audio; nil without timing.
    public var start: TimeInterval?
    public var end: TimeInterval?

    public init(_ text: String, isFinal: Bool = false, utteranceEnded: Bool = false, start: TimeInterval? = nil, end: TimeInterval? = nil) {
        self.text = text
        self.isFinal = isFinal
        self.utteranceEnded = utteranceEnded
        self.start = start
        self.end = end
    }
}

/// Everything the owner said in one dictation (APP-673): each recognition task has its own slot, an ended utterance is
/// kept, the live answer only replaces the words still being heard. When it can't be told whether an answer revises or
/// starts over, the words are kept.
public struct DictationTranscript: Equatable, Sendable {
    struct Slot: Equatable, Sendable {
        var kept: [String] = []
        var live = ""
        var liveEnd: TimeInterval?

        mutating func receive(_ answer: RecognizedSpeech) {
            var incoming = answer.text.trimmingCharacters(in: .whitespacesAndNewlines)
            if !kept.isEmpty, let rest = DictationTranscript.remainder(of: incoming, after: kept.joined(separator: " ")) { incoming = rest }
            if !incoming.isEmpty {
                if !live.isEmpty, DictationTranscript.startedOver(from: live, endingAt: liveEnd, to: incoming, startingAt: answer.start) {
                    kept.append(live)
                }
                live = incoming
                liveEnd = answer.end
            }
            if answer.isFinal || answer.utteranceEnded {
                if !live.isEmpty { kept.append(live) }
                live = ""
                liveEnd = nil
            }
        }
    }

    private var slots: [Slot] = []

    public init() {}

    public var text: String { slots.flatMap { $0.kept + [$0.live] }.filter { !$0.isEmpty }.joined(separator: " ") }

    public mutating func beginTask() -> Int {
        slots.append(Slot())
        return slots.count - 1
    }

    public mutating func receive(_ answer: RecognizedSpeech, task: Int) {
        guard slots.indices.contains(task) else { return }
        slots[task].receive(answer)
    }

    static func words(_ s: String) -> [Substring] { s.split(whereSeparator: { $0.isWhitespace }) }

    static func normalized(_ word: Substring) -> String {
        String(String.UnicodeScalarView(word.lowercased().unicodeScalars.filter(CharacterSet.alphanumerics.contains)))
    }

    /// The words of `text` after `prefix`, when `text` begins with `prefix`'s words (case and punctuation aside).
    static func remainder(of text: String, after prefix: String) -> String? {
        let tokens = words(text), head = words(prefix).map(normalized)
        guard tokens.count >= head.count, zip(tokens, head).allSatisfy({ normalized($0) == $1 }) else { return nil }
        return tokens.dropFirst(head.count).joined(separator: " ")
    }

    /// The answer no longer holds the live words: it starts on audio after they ended, or it is a fraction of them with
    /// a different first word.
    static func startedOver(from live: String, endingAt end: TimeInterval?, to incoming: String, startingAt start: TimeInterval?) -> Bool {
        if let start, let end, end > 0, start >= end { return true }
        let old = words(live).map(normalized), new = words(incoming).map(normalized)
        return new.count * 2 < old.count && new.first != old.first
    }
}

/// One dictation's capture (APP-675): which recognition task takes the audio, and what happens when one ends. Only the
/// owner ends a dictation; a task the recogniser ends while he still holds is replaced, and its words stay.
public struct DictationCapture: Equatable, Sendable {
    public enum Step: Equatable, Sendable {
        case keepListening
        /// Start a new recognition task.
        case restart
        /// The dictation is over, with everything said.
        case finished(String)
        /// A fresh task failed before hearing anything, with nothing said yet.
        case failed
    }

    public private(set) var transcript = DictationTranscript()
    public private(set) var task: Int?
    public private(set) var finishing = false
    private var taskHeard = false

    public init() {}

    public var text: String { transcript.text }

    public mutating func beginTask() -> Int {
        let id = transcript.beginTask()
        task = id
        taskHeard = false
        return id
    }

    public mutating func leaveTask() { task = nil }

    public mutating func receive(_ answer: RecognizedSpeech, task id: Int) -> Step {
        transcript.receive(answer, task: id)
        guard id == task else { return .keepListening }
        taskHeard = true
        return answer.isFinal ? ended() : .keepListening
    }

    public mutating func fail(task id: Int, noSpeech: Bool) -> Step {
        guard id == task else { return .keepListening }
        if finishing || noSpeech || taskHeard { return ended() }
        task = nil
        return text.isEmpty ? .failed : .finished(text)
    }

    public mutating func finish() -> Step {
        finishing = true
        return task == nil ? .finished(text) : .keepListening
    }

    private mutating func ended() -> Step {
        task = nil
        return finishing ? .finished(text) : .restart
    }
}

/// Where this phone turns speech into words: on the phone itself, through the phone's own speech service (on iPhone
/// Apple's servers: it may send the sound of the voice there), or nowhere. kb PHN-604.
public enum SpeechKind: String, Sendable, Equatable { case onDevice = "ON_DEVICE", service = "SERVICE", none = "NONE" }

/// One event from the phone's recogniser for one recognition task.
public enum VoiceEvent: Equatable, Sendable {
    case heard(task: Int, RecognizedSpeech)
    /// `noSpeech`: it heard nothing (a pause); `languageMissing`: no on-device speech for the language.
    case failed(task: Int, noSpeech: Bool, languageMissing: Bool = false, code: Int = 0)
}

/// The phone's recogniser (AppleSpeech in FerbAppUI; a fake in tests), on the main actor. Nothing is recorded: the
/// microphone's sound goes only to the recogniser, never to a file, the store, a log or the Mac.
@MainActor
public protocol VoiceInput: AnyObject {
    func kind() -> SpeechKind
    /// A new recognition task (numbered by the capture) takes the microphone's sound.
    func startTask(kind: SpeechKind, task: Int, events: @escaping @MainActor (VoiceEvent) -> Void)
    /// The owner let go: the current task stops taking sound and gives its last answer.
    func finishAudio()
    /// Everything stops now; no more events.
    func stop()
}

/// One dictation from press to the words left in the message box (the Mac's Microphone.swift, its recogniser behind
/// `VoiceInput`): `words` after each answer, `ended` once. After the owner lets go the last answer completes it, or what
/// was said so far after `grace` seconds.
@MainActor
public final class Dictation {
    public enum End: Equatable, Sendable {
        case words(String)
        case failed(languageMissing: Bool, code: Int)
    }

    /// How long the last answer may take after the owner lets go (the Mac's 2 s).
    public static let grace: TimeInterval = 2

    private let input: VoiceInput
    private let kind: SpeechKind
    private let words: (String) -> Void
    private let ended: (End) -> Void
    private let later: (TimeInterval, @escaping @MainActor () -> Void) -> (() -> Void)
    private var capture = DictationCapture()
    private var over = false
    private var cancelGrace: (() -> Void)?
    private var lastFailure: (languageMissing: Bool, code: Int)?

    public var text: String { capture.text }
    public var running: Bool { !over }

    public init(input: VoiceInput, kind: SpeechKind, words: @escaping (String) -> Void, ended: @escaping (End) -> Void,
                later: @escaping (TimeInterval, @escaping @MainActor () -> Void) -> (() -> Void)) {
        self.input = input
        self.kind = kind
        self.words = words
        self.ended = ended
        self.later = later
    }

    public func start() { startTask() }

    private func startTask() {
        let task = capture.beginTask()
        input.startTask(kind: kind, task: task) { [weak self] e in
            guard let self, !self.over else { return }
            self.event(e)
        }
    }

    private func event(_ e: VoiceEvent) {
        let step: DictationCapture.Step
        switch e {
        case .heard(let task, let speech):
            step = capture.receive(speech, task: task)
            words(capture.text)
        case .failed(let task, let noSpeech, let languageMissing, let code):
            lastFailure = (languageMissing, code)
            step = capture.fail(task: task, noSpeech: noSpeech)
        }
        switch step {
        case .keepListening: break
        case .restart: startTask()
        case .finished(let text): done(.words(text))
        case .failed: done(.failed(languageMissing: lastFailure?.languageMissing ?? false, code: lastFailure?.code ?? 0))
        }
    }

    /// The owner let go.
    public func finish() {
        if over { return }
        if case .finished(let text) = capture.finish() { done(.words(text)); return }
        input.finishAudio()
        cancelGrace = later(Self.grace) { [weak self] in
            guard let self, !self.over else { return }
            self.done(.words(self.capture.text))
        }
    }

    /// Leaving the app, the lock, a new dictation: stops now, keeping what was said.
    public func cancel() { if !over { done(.words(capture.text)) } }

    private func done(_ end: End) {
        over = true
        cancelGrace?()
        cancelGrace = nil
        input.stop()
        ended(end)
    }
}
