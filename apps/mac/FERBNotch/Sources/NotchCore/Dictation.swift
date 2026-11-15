import Foundation

/// One answer of the speech recognizer, as plain facts (NotchCore never imports Speech).
public struct RecognizedSpeech: Equatable, Sendable {
    public var text: String
    /// The recognition task is done: nothing more comes from it.
    public var isFinal: Bool
    /// The recognizer marked the end of an utterance (`speechRecognitionMetadata`): after a pause, its later answers can
    /// start again without these words.
    public var utteranceEnded: Bool
    /// Where the first word starts and the last word ends, in seconds of the task's audio; nil when the recognizer gave
    /// no timing.
    public var start: TimeInterval?
    public var end: TimeInterval?

    public init(_ text: String, isFinal: Bool = false, utteranceEnded: Bool = false,
                start: TimeInterval? = nil, end: TimeInterval? = nil) {
        self.text = text
        self.isFinal = isFinal
        self.utteranceEnded = utteranceEnded
        self.start = start
        self.end = end
    }
}

/// Everything the owner said in one dictation (APP-673). Apple Speech answers with its best transcription so far, but
/// after a pause it can end an utterance, finish its task, or start a new answer that no longer holds the earlier
/// words; replacing the transcript with the latest answer lost them. Here every recognition task has its own slot, an
/// ended utterance is kept, and the live answer only ever replaces the words of the utterance still being heard.
/// When it can't be told whether an answer revises or starts over, the words are kept: a repeated word in the draft
/// the owner reviews costs less than a lost one.
public struct DictationTranscript: Equatable, Sendable {
    struct Slot: Equatable, Sendable {
        var kept: [String] = []
        var live = ""
        var liveEnd: TimeInterval?
    }

    private var slots: [Slot] = []

    public init() {}

    public var text: String {
        slots.flatMap { $0.kept + [$0.live] }.filter { !$0.isEmpty }.joined(separator: " ")
    }

    /// A new recognition task: its words go after every earlier task's.
    public mutating func beginTask() -> Int {
        slots.append(Slot())
        return slots.count - 1
    }

    public mutating func receive(_ answer: RecognizedSpeech, task: Int) {
        guard slots.indices.contains(task) else { return }
        slots[task].receive(answer)
    }
}

extension DictationTranscript.Slot {
    mutating func receive(_ answer: RecognizedSpeech) {
        var incoming = answer.text.trimmingCharacters(in: .whitespacesAndNewlines)
        // A recognizer that repeats the utterances it already ended at the front of later answers: only the rest is new.
        if !kept.isEmpty, let rest = Self.remainder(of: incoming, after: kept.joined(separator: " ")) { incoming = rest }
        if !incoming.isEmpty {
            if !live.isEmpty, Self.startedOver(from: live, endingAt: liveEnd, to: incoming, startingAt: answer.start) {
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

    /// The words of `text` after `prefix`, when `text` begins with `prefix`'s words (case and punctuation aside).
    static func remainder(of text: String, after prefix: String) -> String? {
        let tokens = text.split(whitespace: ()), head = prefix.split(whitespace: ()).map(normalized)
        guard tokens.count >= head.count, zip(tokens, head).allSatisfy({ normalized($0) == $1 }) else { return nil }
        return tokens.dropFirst(head.count).joined(separator: " ")
    }

    /// The answer no longer holds the live words: it starts on audio after they ended, or it is a fraction of them
    /// with a different first word (a revision rewrites the last words, not the whole utterance at once).
    static func startedOver(from live: String, endingAt end: TimeInterval?, to incoming: String,
                            startingAt start: TimeInterval?) -> Bool {
        if let start, let end, end > 0, start >= end { return true }
        let old = live.split(whitespace: ()).map(normalized), new = incoming.split(whitespace: ()).map(normalized)
        return new.count * 2 < old.count && new.first != old.first
    }

    static func normalized(_ word: Substring) -> String {
        String(String.UnicodeScalarView(word.lowercased().unicodeScalars.filter(CharacterSet.alphanumerics.contains)))
    }
}

private extension String {
    func split(whitespace: Void) -> [Substring] { split(whereSeparator: { $0.isWhitespace }) }
}

/// One dictation's capture (APP-673): which recognition task takes the audio, and what happens when one ends. Only the
/// owner ends a dictation (release, Stop, Cancel, hiding, typing: APP-568); a task the recognizer ends while the owner
/// is still dictating (its final answer after a pause, "no speech detected", an error after it heard words) is
/// replaced by a new one on the same running microphone, and its words stay.
public struct DictationCapture: Equatable, Sendable {
    public enum Step: Equatable, Sendable {
        case keepListening
        /// Start a new recognition task on the running microphone.
        case restart
        /// The dictation is over, with everything said.
        case finished(String)
        /// A fresh task failed before hearing anything, with nothing said yet: the error is the answer.
        case failed
    }

    public private(set) var transcript = DictationTranscript()
    /// The recognition task the microphone feeds now; nil between tasks.
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

    /// The task stops being fed (an audio route change rebuilds the microphone); its words stay and any it still sends
    /// are kept.
    public mutating func leaveTask() { task = nil }

    public mutating func receive(_ answer: RecognizedSpeech, task id: Int) -> Step {
        transcript.receive(answer, task: id)
        guard id == task else { return .keepListening }
        taskHeard = true
        return answer.isFinal ? ended() : .keepListening
    }

    /// The task ended with an error; `noSpeech`: the recognizer heard no speech in its audio (the owner paused).
    public mutating func fail(task id: Int, noSpeech: Bool) -> Step {
        guard id == task else { return .keepListening }
        if finishing || noSpeech || taskHeard { return ended() }
        // A new task that fails at once would fail again: stop, keeping what was said.
        task = nil
        return text.isEmpty ? .failed : .finished(text)
    }

    /// The owner finished: the current task's last answer completes the dictation.
    public mutating func finish() -> Step {
        finishing = true
        return task == nil ? .finished(text) : .keepListening
    }

    private mutating func ended() -> Step {
        task = nil
        return finishing ? .finished(text) : .restart
    }
}

/// The dictation meter (APP-674): the microphone's loudness, smoothed (fast attack, slower release), and its recent
/// history, newest first. Silence (at or below `quietest`) is 0, so the meter is still; only the voice moves it.
public struct VoiceLevel: Equatable, Sendable {
    /// dBFS mapped to 0 and 1.
    public static let quietest = -50.0, loudest = -18.0
    /// Smoothing time constants, in seconds of audio.
    public static let attack = 0.03, release = 0.3

    public private(set) var level = 0.0
    public private(set) var history: [Double]

    public init(length: Int = 25) { history = Array(repeating: 0, count: max(1, length)) }

    /// 0...1 from a block's RMS amplitude (full scale 1).
    public static func loudness(rms: Double) -> Double {
        guard rms.isFinite, rms > 0 else { return 0 }
        return min(1, max(0, (20 * log10(rms) - quietest) / (loudest - quietest)))
    }

    /// RMS of each `chunk` samples (a short last chunk included).
    public static func rms(_ samples: UnsafeBufferPointer<Float>, chunk: Int) -> [Double] {
        guard chunk > 0, !samples.isEmpty else { return [] }
        return stride(from: 0, to: samples.count, by: chunk).map { start in
            let part = samples[start..<min(start + chunk, samples.count)]
            return (part.reduce(0.0) { $0 + Double($1) * Double($1) } / Double(part.count)).squareRoot()
        }
    }

    /// One block of audio `duration` seconds long.
    public mutating func add(rms: Double, duration: TimeInterval) {
        let target = Self.loudness(rms: rms)
        if duration > 0 {
            level += (target - level) * (1 - exp(-duration / (target > level ? Self.attack : Self.release)))
        }
        if level < 0.002 { level = 0 }
        history.removeLast()
        history.insert(level, at: 0)
    }
}
