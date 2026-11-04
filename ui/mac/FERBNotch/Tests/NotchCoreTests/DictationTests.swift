import XCTest
import NotchCore
@testable import FERBNotch

/// Build 202611040231: dictation cut the message after a pause, and its waveform moved without any voice.
final class DictationTests: XCTestCase {
    // decision: APP-673
    func testWordsFinalizedBeforeAPauseAreKeptWhenTheRecognizerStartsOver() {
        var capture = DictationCapture()
        let task = capture.beginTask()
        XCTAssertEqual(capture.receive(RecognizedSpeech("Find my"), task: task), .keepListening)
        XCTAssertEqual(capture.receive(RecognizedSpeech("Find my notes", utteranceEnded: true), task: task), .keepListening)
        // After the pause the recognizer's answers no longer hold the earlier words.
        _ = capture.receive(RecognizedSpeech("about"), task: task)
        _ = capture.receive(RecognizedSpeech("about the launch"), task: task)
        XCTAssertEqual(capture.text, "Find my notes about the launch")
    }

    // decision: APP-673
    func testARecognizerThatRepeatsEarlierWordsIsNotDoubled() {
        var capture = DictationCapture()
        let task = capture.beginTask()
        _ = capture.receive(RecognizedSpeech("Find my notes.", utteranceEnded: true), task: task)
        _ = capture.receive(RecognizedSpeech("Find my notes, about the launch"), task: task)
        XCTAssertEqual(capture.text, "Find my notes. about the launch")
    }

    // decision: APP-673
    func testAnAnswerThatDropsTheLiveWordsWithoutMarkingThePauseKeepsThem() {
        var capture = DictationCapture()
        let task = capture.beginTask()
        _ = capture.receive(RecognizedSpeech("please book a table for four", start: 0.2, end: 2.1), task: task)
        _ = capture.receive(RecognizedSpeech("tonight", start: 4.0, end: 4.5), task: task)     // after the old words
        _ = capture.receive(RecognizedSpeech("tonight at", start: 4.0, end: 4.8), task: task)
        XCTAssertEqual(capture.text, "please book a table for four tonight at")
        // A revision of the same words replaces them.
        var revised = DictationCapture()
        let other = revised.beginTask()
        _ = revised.receive(RecognizedSpeech("I scream", start: 0.1, end: 0.9), task: other)
        _ = revised.receive(RecognizedSpeech("Ice cream", start: 0.1, end: 0.9), task: other)
        XCTAssertEqual(revised.text, "Ice cream")
    }

    // decision: APP-675, APP-673, APP-568
    func testTheRecognizerEndingMidHoldRestartsAndKeepsEverything() {
        var capture = DictationCapture()
        let first = capture.beginTask()
        _ = capture.receive(RecognizedSpeech("Send the report"), task: first)
        // A final answer after a pause no longer ends the dictation: a new task takes the audio.
        XCTAssertEqual(capture.receive(RecognizedSpeech("Send the report.", isFinal: true), task: first), .restart)
        let second = capture.beginTask()
        // "No speech detected" while the owner is quiet: another task.
        XCTAssertEqual(capture.fail(task: second, noSpeech: true), .restart)
        let third = capture.beginTask()
        _ = capture.receive(RecognizedSpeech("to Sam"), task: third)
        XCTAssertEqual(capture.fail(task: third, noSpeech: false), .restart)     // an error after it heard words
        let fourth = capture.beginTask()
        _ = capture.receive(RecognizedSpeech("by Friday"), task: fourth)
        // Only the owner's release ends it, with the last task's final answer.
        XCTAssertEqual(capture.finish(), .keepListening)
        XCTAssertEqual(capture.receive(RecognizedSpeech("by Friday.", isFinal: true), task: fourth),
                       .finished("Send the report. to Sam by Friday."))
    }

    // decision: APP-675
    func testAFreshTaskThatFailsAtOnceStopsWithWhatWasSaid() {
        var capture = DictationCapture()
        let first = capture.beginTask()
        _ = capture.receive(RecognizedSpeech("Open my calendar", isFinal: true), task: first)
        let second = capture.beginTask()
        XCTAssertEqual(capture.fail(task: second, noSpeech: false), .finished("Open my calendar"))
        var empty = DictationCapture()
        let only = empty.beginTask()
        XCTAssertEqual(empty.fail(task: only, noSpeech: false), .failed)
    }

    // decision: APP-673
    func testAnOldTasksLateWordsStayAndNeverEndTheDictation() {
        var capture = DictationCapture()
        let old = capture.beginTask()
        _ = capture.receive(RecognizedSpeech("Remind me"), task: old)
        capture.leaveTask()                                        // an audio route change rebuilt the microphone
        let new = capture.beginTask()
        _ = capture.receive(RecognizedSpeech("at noon"), task: new)
        XCTAssertEqual(capture.receive(RecognizedSpeech("Remind me tomorrow", isFinal: true), task: old), .keepListening)
        XCTAssertEqual(capture.text, "Remind me tomorrow at noon")
    }

    // decision: APP-674
    func testSilenceKeepsTheMeterStillAndVoiceMovesIt() {
        var level = VoiceLevel()
        for _ in 0..<50 { level.add(rms: 0.0005, duration: 0.02) }        // a quiet room, about -66 dBFS
        XCTAssertEqual(level.history, [Double](repeating: 0, count: level.history.count))
        let still = (0..<VoiceLight.bars).map { VoiceLight.height($0, history: level.history) }
        XCTAssertEqual(Set(still), [VoiceLight.quietHeight])

        // Syllables: loud blocks with gaps.
        for block in 0..<40 { level.add(rms: block % 8 < 5 ? 0.05 : 0.002, duration: 0.02) }
        XCTAssertGreaterThan(level.history.max() ?? 0, 0.5)
        XCTAssertGreaterThan(Set(level.history).count, 5)
        let moving = (0..<VoiceLight.bars).map { VoiceLight.height($0, history: level.history) }
        XCTAssertGreaterThan(moving.max() ?? 0, VoiceLight.quietHeight + 5)
    }

    // decision: APP-674
    func testTheMeterRisesFastAndFallsSlower() {
        var level = VoiceLevel()
        level.add(rms: 0.05, duration: 0.02)
        level.add(rms: 0.05, duration: 0.02)
        let peak = VoiceLevel.loudness(rms: 0.05)
        XCTAssertGreaterThan(level.level, peak * 0.7)                    // two 20 ms blocks reach most of the voice
        level.add(rms: 0, duration: 0.02)
        level.add(rms: 0, duration: 0.02)
        XCTAssertGreaterThan(level.level, peak * 0.6)                    // the same 40 ms of silence keeps most of it
        for _ in 0..<100 { level.add(rms: 0, duration: 0.02) }
        XCTAssertEqual(level.level, 0)                                    // and it settles to still
    }

    // decision: APP-674
    func testBlockRMS() {
        let samples: [Float] = [0.5, -0.5, 0.5, -0.5, 0, 0]
        let values = samples.withUnsafeBufferPointer { VoiceLevel.rms($0, chunk: 4) }
        XCTAssertEqual(values, [0.5, 0])
    }
}
