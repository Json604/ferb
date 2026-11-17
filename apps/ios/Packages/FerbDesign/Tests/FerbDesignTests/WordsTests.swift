import Foundation
import XCTest
@testable import FerbDesign

/// The home screen's words (Words.swift and words/corpus.json), as Android's app WordsTest holds its port.
final class WordsTests: XCTestCase {
    private let corpus = Corpus.bundled

    private func moment(_ hour: Double, firstRun: Bool = false, waiting: Int = 0, link: String = "direct", justPaired: Bool = false,
                        lastOpen: Date? = nil) -> Moment {
        Moment(sky: .lab(hour), waiting: waiting, link: link, firstRun: firstRun, justPaired: justPaired, lastOpen: lastOpen,
               lastZone: "Asia/Kolkata")
    }

    func testTheCorpusIsTheLabsByteForByte() throws {
        let url = try XCTUnwrap(Corpus.bundledURL)
        let android = LabNumbersTests.repo.appendingPathComponent("apps/android/app/src/main/assets/corpus.json")
        XCTAssertEqual(try Data(contentsOf: url), try Data(contentsOf: android), "the two phones speak the same corpus")
        XCTAssertFalse(corpus.rules.isEmpty)
    }

    // decision: PHN-507
    func testTheFirstRunAndTheFirstOpenAfterPairingHaveTheirOwnWords() {
        let first = HomeTexts(moment(10, firstRun: true))
        XCTAssertTrue(["Hello", "Hi, I'm FERB"].contains(first.greeting), first.greeting)
        XCTAssertEqual(first.subline, "Pair with your Mac and ask for anything. It happens there.")
        let paired = HomeTexts(moment(10, justPaired: true))
        XCTAssertTrue(["All set", "You're connected"].contains(paired.greeting), paired.greeting)
    }

    // decision: PHN-507
    func testAMacThatCantBeReachedIsSaidAndTheSameLineHoldsAllDay() {
        let a = HomeTexts(moment(9, link: "offline"))
        XCTAssertTrue(a.subline.contains("Mac"), a.subline)
        XCTAssertEqual(a.subline, HomeTexts(moment(16, link: "offline")).subline)
    }

    // decision: PHN-507, PHN-412
    func testConditionsThePhoneDoesNotMeasureNeverHold() {
        // weather is paused (PHN-412) and the owner's habits are not kept: no rule on them can speak
        XCTAssertFalse(Corpus.holds("weather", "storm", nil))
        let values = moment(10, lastOpen: Date(timeIntervalSince1970: 1_763_000_000)).values
        for unmeasured in ["weather", "temp", "rainIn", "usual", "lateNights"] { XCTAssertNil(values[unmeasured], unmeasured) }
        XCTAssertTrue(Corpus.holds("hour", "22-4", 23.5))
        XCTAssertTrue(Corpus.holds("hour", "22-4", 3.0))
        XCTAssertFalse(Corpus.holds("hour", "22-4", 4.0))
        XCTAssertTrue(Corpus.holds("awayHours", ">=168", 200.0))
        XCTAssertTrue(Corpus.holds("phase", ["dawn", "sunrise"], "dawn"))
        XCTAssertTrue(Corpus.holds("firstRun", true as NSNumber, true))
        XCTAssertFalse(Corpus.holds("waiting", true as NSNumber, 1))
        XCTAssertNil(corpus.line("nothing", moment(10)))
        XCTAssertFalse(HomeTexts(moment(2)).greeting.isEmpty)
    }

    /// The open before this one decides "first today" (the day starts at 4 am) and the away time.
    func testTheOpenBeforeThisOneIsMeasured() {
        let m = moment(9)
        let lastNight = m.sky.date.addingTimeInterval(-6 * 3600)  // 3 am: before today's 4 am
        let earlier = m.sky.date.addingTimeInterval(-3600)
        XCTAssertEqual(moment(9, lastOpen: lastNight).values["firstToday"] as? Bool, true)
        XCTAssertEqual(moment(9, lastOpen: earlier).values["firstToday"] as? Bool, false)
        XCTAssertEqual(moment(9, lastOpen: earlier).values["awayHours"] as? Double ?? 0, 1, accuracy: 1e-9)
        XCTAssertEqual(moment(9, lastOpen: earlier).fills["lastOpen"], "8:00 AM")
        XCTAssertNil(moment(9).values["firstToday"])
    }

    /// The day's line: Calendar's ordinal of the day in the era, as the lab and Android pick it (0001-01-01 is day 1).
    func testTheLineIsPickedByTheDaysOrdinal() {
        XCTAssertEqual(moment(12).dayNumber, 739_934)
    }

    /// A time left: "45 min" below an hour, "5 h" for whole hours (the owner's "5h", not the lab's "5 h 0 min"), "5 h
    /// 20 min" otherwise; the table is Android's (LabWordsTest), so the two phones can't drift.
    // decision: PHN-595
    func testTimeLeftDropsZeroMinutesAsTheOwnerChose() throws {
        let url = LabNumbersTests.repo.appendingPathComponent("apps/android/design/src/test/resources/words/time-left.json")
        let table = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
        let cases = try XCTUnwrap(table["cases"] as? [[Any]])
        XCTAssertGreaterThanOrEqual(cases.count, 6)
        for c in cases {
            let minutes = try XCTUnwrap(c[0] as? Int), words = try XCTUnwrap(c[1] as? String)
            XCTAssertEqual(labMinutesLeft(minutes), words, "\(minutes)")
        }
    }

    /// The relay sheet promises only what the shared relay carries.
    // decision: PHN-410, PHN-444
    func testTheLabsWordsAndTheRelaySheet() {
        let how = Dictionary(uniqueKeysWithValues: LabSample.relay.works.map { ($0.what, $0.how) })
        XCTAssertEqual(how["Messages, questions, answers, Stop"], "Always")
        XCTAssertEqual(how["Files and pictures"], "Not on the shared relay")
        XCTAssertEqual(how["Watch live"], "Not on the shared relay")
        XCTAssertEqual(HomeTexts.lab(.lab(6.1)).subline, "Sunrise at 6:45 AM.")
    }
}
